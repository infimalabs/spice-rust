use std::collections::BTreeSet;
use std::io::{self, Read as _, Write as _};
use std::net::{Shutdown, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gate::test_support::gix;

use super::*;
mod multi_worktree;

#[test]
fn executable_rollback_requires_the_journal_schema_to_be_proven_unchanged() {
    const CURRENT: u32 = spice::journal_schema::CURRENT;
    assert!(schema_rollback_safe(None, None));
    assert!(schema_rollback_safe(Some(CURRENT), Some(CURRENT)));
    assert!(!schema_rollback_safe(None, Some(CURRENT)));
    assert!(!schema_rollback_safe(Some(CURRENT - 1), Some(CURRENT)));
    assert!(!schema_rollback_safe(Some(CURRENT), None));
}

#[test]
fn a_source_stamp_orders_only_a_commit_later_than_its_first_parent() {
    let stamp = |log| stamp_from_committer_seconds(log);
    let head = stamp("1790000001\n1790000000\n").expect("later commit stamps");
    assert_eq!(
        head,
        TaskStamp::for_micros(1_790_000_001_000_000).expect("stamp")
    );
    assert_eq!(
        stamp("1790000001\n"),
        Ok(head),
        "a root commit has no parent to precede"
    );
    for refused in [
        "1790000000\n1790000000\n",
        "1790000000\n1790000001\n",
        "",
        "HEAD\n",
    ] {
        assert!(stamp(refused).is_err(), "{refused:?} must not stamp");
    }
}

#[test]
fn no_resident_refresh_does_not_inspect_journal_schema() {
    let fixture = Fixture::new("no-resident-schema");
    let repository = fixture.repository();
    let journal = repository
        .admit_state()
        .expect("repository state")
        .journal()
        .to_path_buf();
    fs::create_dir_all(journal.parent().expect("journal parent")).expect("journal parent exists");
    fs::write(&journal, b"not a SQLite journal").expect("invalid journal writes");

    assert_eq!(
        schema_recovery_boundary(&repository, false).expect("no-resident boundary skips"),
        None,
    );
    assert!(schema_recovery_boundary(&repository, true).is_err());
}

#[test]
fn a_committed_schema_advance_preserves_the_successor_executable() {
    let fixture = Fixture::new("schema-advance");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let predecessor = fixture.binary("spice-predecessor", b"predecessor\n");
    let successor = fixture.binary("spice-successor", b"successor\n");
    refresh_install(&repository, &predecessor).expect("predecessor installs");
    let parked = park(&repository, &installed)
        .expect("predecessor parks")
        .expect("predecessor exists");
    refresh_install(&repository, &successor).expect("successor installs");
    let rollback = Rollback {
        installed: &installed,
        executable: Some(&parked),
        resident_executable: None,
        published: None,
        restart: false,
    };
    let mut output = Vec::new();

    const SUCCESSOR_SCHEMA: u32 = spice::journal_schema::CURRENT;
    let error = recover_start_failure_after(
        &repository,
        &rollback,
        Some(SUCCESSOR_SCHEMA - 1),
        Ok(Some(SUCCESSOR_SCHEMA)),
        &mut output,
        RefreshError::ResidentUnready,
    );

    assert!(matches!(error, RefreshError::SchemaAdvanced { .. }));
    assert_eq!(
        fs::read(installed).expect("installed reads"),
        b"successor\n"
    );
    assert!(String::from_utf8_lossy(&output).contains("preserving new executable"));
}

#[test]
fn uncertain_schema_recovery_preserves_the_successor_and_names_both_causes() {
    let fixture = Fixture::new("schema-uncertain");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let predecessor = fixture.binary("spice-predecessor", b"predecessor\n");
    let successor = fixture.binary("spice-successor", b"successor\n");
    refresh_install(&repository, &predecessor).expect("predecessor installs");
    let parked = park(&repository, &installed)
        .expect("predecessor parks")
        .expect("predecessor exists");
    refresh_install(&repository, &successor).expect("successor installs");
    let invalid = fixture.0.join("uncertain-journal.sqlite3");
    fs::write(&invalid, b"not a SQLite authority").expect("uncertain journal writes");
    let inspection = spice::journal_schema::inspect(&invalid)
        .expect_err("invalid journal makes recovery uncertain");
    let mut output = Vec::new();

    let error = recover_start_failure_after(
        &repository,
        &Rollback {
            installed: &installed,
            executable: Some(&parked),
            resident_executable: None,
            published: None,
            restart: false,
        },
        Some(spice::journal_schema::CURRENT - 1),
        Err(inspection),
        &mut output,
        RefreshError::ResidentUnready,
    );

    assert!(matches!(
        error,
        RefreshError::SchemaRecoveryUncertain {
            before: Some(before),
            ref cause,
            ..
        } if before == spice::journal_schema::CURRENT - 1
            && cause.contains("resident never became ready")
    ));
    assert_eq!(
        fs::read(installed).expect("installed successor reads"),
        b"successor\n",
        "uncertain recovery never republishes an older schema writer",
    );
    assert!(
        output.is_empty(),
        "uncertain recovery performs no rollback action"
    );
}

/// How many names one fixture tries before giving up on an exclusive root.
const SCRATCH_ATTEMPTS: usize = 100;

/// A scratch repository the installer is allowed to publish into.
struct Fixture(PathBuf);

impl Fixture {
    /// One scratch repository under a root nothing else already holds.
    ///
    /// `create_dir` refuses a directory that exists rather than opening it, so
    /// retrying under a fresh name is what makes the root exclusive. The name
    /// carries the clock rather than the process id because ids are reused and
    /// the clock is not: a run that inherited a recycled id used to find its own
    /// name already taken by a root an earlier run had left behind.
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..SCRATCH_ATTEMPTS {
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("the clock reads after the epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "spice-xtask-develop-{label}-{stamp}-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self::initialized(root),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("exclusive scratch root is created: {error}"),
            }
        }
        panic!("no exclusive scratch root was available in {SCRATCH_ATTEMPTS} attempts");
    }

    /// Canonicalize the freshly created `root` and make it a repository.
    fn initialized(root: PathBuf) -> Self {
        let fixture = Self(fs::canonicalize(&root).expect("scratch root canonicalizes"));
        let repository = gix::ThreadSafeRepository::init_opts(
            &fixture.0,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::isolated(),
        )
        .expect("initialize in-process scratch repository")
        .to_thread_local();
        fs::write(repository.git_dir().join("HEAD"), "ref: refs/heads/main\n")
            .expect("select deterministic scratch branch");
        seed_canvas_stage(&fixture.0);
        fixture
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.0).expect("scratch repository resolves")
    }

    /// Write one distinct executable image and answer where it landed.
    fn binary(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).expect("scratch executable is written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("scratch executable is made executable");
        }
        path
    }

    fn publish(&self, endpoint: &str) -> spice::canvas::Discovery {
        let identity = "00000000-0000-4000-8000-000000000000"
            .parse::<wire::CanonicalUuid>()
            .expect("fixture identity is canonical");
        let discovery = spice::canvas::Discovery {
            endpoint: endpoint.to_owned(),
            capability: identity,
            epoch: identity,
        };
        let state = self
            .repository()
            .admit_state()
            .expect("scratch repository state is admitted");
        if let Some(parent) = state.canvas().parent() {
            fs::create_dir_all(parent).expect("scratch discovery parent is created");
        }
        discovery
            .publish(state.canvas())
            .expect("scratch discovery publishes");
        discovery
    }
}

fn seed_canvas_stage(worktree: &Path) {
    let generation = wire::Digest::compute(wire::Domain::DistInventory, b"xtask fixture app");
    let stage = worktree.join("dist/stage-app");
    let root = stage.join("generations").join(generation.as_str());
    fs::create_dir_all(&root).expect("create fixture Canvas generation");
    for file in wire::manifest::CANVAS_APP_FILES {
        fs::write(root.join(file), format!("xtask Canvas asset {file}\n"))
            .expect("write fixture Canvas asset");
    }
    fs::write(stage.join("current"), format!("{generation}\n"))
        .expect("write fixture Canvas pointer");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // A root that outlives its test is a name the next run can collide
        // with, so remove it even where a panic is unwinding through here.
        // Removal is allowed to fail: a resident this test started may still
        // hold a file open, and reporting that from a drop would replace the
        // failure the test is already carrying.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A port nothing is bound to, taken by binding one and letting it go.
///
/// Naming a fixed port would race every other lane on this machine, and the
/// distinction under test is exactly whether the endpoint answers.
fn unbound_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a scratch port binds");
    let port = listener
        .local_addr()
        .expect("the scratch port is named")
        .port();
    drop(listener);
    port
}

/// Serve one exact resident bootstrap from `listener`.
fn serve_bootstrap(
    listener: TcpListener,
    discovery: spice::canvas::Discovery,
    binary: Digest,
) -> thread::JoinHandle<()> {
    serve_bootstraps(listener, discovery, vec![binary])
}

/// Serve successive resident generations from one stable endpoint.
fn serve_bootstraps(
    listener: TcpListener,
    discovery: spice::canvas::Discovery,
    binaries: Vec<Digest>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for binary in binaries {
            let bootstrap = wire::CanvasBootstrap {
                endpoint: discovery.endpoint.clone(),
                capability: discovery.capability,
                epoch: discovery.epoch,
                version: wire::Version {
                    product: wire::manifest::ProductVersion::parse("0.2.0-dev.0")
                        .expect("fixture product version"),
                    binary,
                    wire: wire::WireVersion::CURRENT,
                    journal: spice::journal_schema::CURRENT,
                    canvas: wire::CanvasVersion::CURRENT,
                    app: Digest::compute(wire::Domain::DistInventory, b"fixture app"),
                },
                operator: wire::OperatorIdentity::default(),
            };
            let body = wire::encode(&bootstrap).expect("fixture bootstrap encodes");
            let (mut stream, _) = listener.accept().expect("bootstrap connection arrives");
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0_u8; 256];
                let read = stream.read(&mut chunk).expect("bootstrap request reads");
                assert_ne!(read, 0, "bootstrap request terminates its headers");
                request.extend_from_slice(&chunk[..read]);
                assert!(request.len() <= 4 * 1024, "bootstrap request stays bounded");
            }
            assert!(
                request.starts_with(b"GET /bootstrap HTTP/1.1\r\n"),
                "unexpected bootstrap request: {}",
                String::from_utf8_lossy(&request)
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .and_then(|()| stream.shutdown(Shutdown::Write))
            .expect("bootstrap response writes");
        }
    })
}

/// The exact command sequence that exposed the principal mismatch.
///
/// The first publication leaves its predecessor resident live. The second must
/// execute `down` through the retained predecessor, then launch the installed
/// successor and expose that successor's digest at the same endpoint.
#[test]
fn no_resident_publication_then_restart_uses_the_retained_predecessor_authority() {
    let fixture = Fixture::new("two-install-predecessor");
    let repository = fixture.repository();
    let state = repository.admit_state().expect("fixture state is admitted");
    let discovery_path = state.canvas().to_path_buf();
    let stopped = fixture.0.join("predecessor-stopped");
    let started = fixture.0.join("successor-started");
    let saved_discovery = fixture.0.join("saved-discovery");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let address = listener.local_addr().expect("bootstrap address reads");
    let endpoint = format!("http://{address}");

    let predecessor = fixture.binary(
        "spice-predecessor",
        format!(
            "#!/bin/sh\nif [ \"$1\" = down ]; then rm -f '{}'; : > '{}'; exit 0; fi\necho 'unexpected predecessor command' >&2\nexit 9\n",
            discovery_path.display(),
            stopped.display(),
        )
        .as_bytes(),
    );
    refresh_install(&repository, &predecessor).expect("predecessor installs");
    let predecessor_binary =
        executable_digest(&install::installed_binary(&repository)).expect("predecessor hashes");
    let discovery = fixture.publish(&endpoint);
    fs::copy(&discovery_path, &saved_discovery).expect("discovery is retained for successor");
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: endpoint.clone(),
            epoch: discovery.epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let successor = fixture.binary(
        "spice-successor",
        format!(
            "#!/bin/sh\ncp '{}' '{}'\n: > '{}'\nprintf '%s\\n' '{}'\n",
            saved_discovery.display(),
            discovery_path.display(),
            started.display(),
            readiness,
        )
        .as_bytes(),
    );
    let successor_binary = executable_digest(&successor).expect("successor hashes");
    let server = serve_bootstraps(
        listener,
        discovery,
        vec![
            predecessor_binary.clone(),
            predecessor_binary,
            successor_binary.clone(),
        ],
    );
    let no_resident = RefreshRequest {
        restart: false,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    install_built(&repository, successor.clone(), &no_resident, &mut output)
        .expect("no-resident publication installs successor");
    assert!(
        !stopped.exists(),
        "no-resident publication stopped its predecessor"
    );
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };

    let report = install_built(&repository, successor, &request, &mut output)
        .expect("second install replaces the live predecessor");
    let (_, running) = running_resident_version(&repository).expect("successor bootstrap answers");
    server.join().expect("bootstrap server joins");

    assert!(stopped.exists(), "retained predecessor did not stop itself");
    assert!(started.exists(), "installed successor did not launch");
    assert_eq!(report.endpoint.as_deref(), Some(endpoint.as_str()));
    assert_eq!(
        running.binary, successor_binary,
        "endpoint did not report the installed successor"
    );
}

/// A stable-rollback promotion fault occurs after `down`, so it must restore
/// service through the separately retained resident authority before returning.
#[test]
fn rollback_promotion_failure_restores_the_predecessor_resident() {
    let fixture = Fixture::new("rollback-promotion-failure");
    let repository = fixture.repository();
    let state = repository.admit_state().expect("fixture state is admitted");
    let discovery_path = state.canvas().to_path_buf();
    let saved_discovery = fixture.0.join("saved-discovery");
    let restored = fixture.0.join("predecessor-restored");
    let common = workspace(&repository).expect("common refresh workspace exists");
    let stable_rollback = common.join(ROLLBACK);
    let retained_resident = common.join(RESIDENT_ROLLBACK);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let address = listener.local_addr().expect("bootstrap address reads");
    let endpoint = format!("http://{address}");
    let discovery = fixture.publish(&endpoint);
    fs::copy(&discovery_path, &saved_discovery).expect("discovery is retained for recovery");
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: endpoint.clone(),
            epoch: discovery.epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let predecessor = fixture.binary(
        "spice-predecessor",
        format!(
            "#!/bin/sh\nif [ \"$1\" = down ]; then rm -f '{}'; rm -f '{}'; mkdir '{}'; exit 0; fi\ncp '{}' '{}'\n: > '{}'\nprintf '%s\\n' '{}'\n",
            discovery_path.display(),
            stable_rollback.display(),
            stable_rollback.display(),
            saved_discovery.display(),
            discovery_path.display(),
            restored.display(),
            readiness,
        )
        .as_bytes(),
    );
    refresh_install(&repository, &predecessor).expect("predecessor installs");
    let predecessor_binary =
        executable_digest(&install::installed_binary(&repository)).expect("predecessor hashes");
    let successor = fixture.binary("spice-successor", b"successor executable\n");
    let server = serve_bootstraps(
        listener,
        discovery,
        vec![predecessor_binary.clone(), predecessor_binary.clone()],
    );
    let no_resident = RefreshRequest {
        restart: false,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();
    install_built(&repository, successor.clone(), &no_resident, &mut output)
        .expect("no-resident publication installs successor");
    let installed = install::installed_binary(&repository);
    let successor_bytes = fs::read(&installed).expect("successor reads");
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };

    let result = install_built(&repository, successor, &request, &mut output);
    server.join().expect("bootstrap server joins");

    assert!(
        matches!(result, Err(RefreshError::Io(_))),
        "rollback promotion fault was not reported: {result:?}"
    );
    assert!(
        restored.exists() && discovery_path.is_file(),
        "the predecessor resident was not restored: {}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(
        fs::read(installed).expect("installed successor still reads"),
        successor_bytes,
        "promotion failure changed the previously runnable executable"
    );
    assert_eq!(
        executable_digest(&retained_resident).expect("retained authority hashes"),
        predecessor_binary,
        "promotion failure lost the complete predecessor authority"
    );
}

/// An authenticated generation outside both transaction slots is not ours.
#[test]
fn a_foreign_resident_is_refused_before_stop_or_install() {
    let fixture = Fixture::new("foreign-resident");
    let repository = fixture.repository();
    let invoked = fixture.0.join("installed-command-invoked");
    let installed_source = fixture.binary(
        "spice-installed",
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", invoked.display()).as_bytes(),
    );
    refresh_install(&repository, &installed_source).expect("current executable installs");
    let installed = install::installed_binary(&repository);
    let before = fs::read(&installed).expect("installed executable reads");
    let successor = fixture.binary("spice-foreign-successor", b"successor executable\n");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let address = listener.local_addr().expect("bootstrap address reads");
    let discovery = fixture.publish(&format!("http://{address}"));
    let foreign = Digest::compute(wire::Domain::DistArtifact, b"foreign executable");
    let server = serve_bootstrap(listener, discovery, foreign);
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();
    let started_at = Instant::now();

    let result = install_built(&repository, successor, &request, &mut output);
    let elapsed = started_at.elapsed();
    server.join().expect("bootstrap server joins");

    assert!(
        matches!(&result, Err(RefreshError::ResidentStopRefused(refusal))
            if refusal.contains("resident::foreign-generation")),
        "foreign resident refusal was not named: {result:?}"
    );
    assert!(
        !invoked.exists(),
        "foreign resident was sent a stop command"
    );
    assert_eq!(
        fs::read(installed).expect("installed executable still reads"),
        before,
        "foreign resident refusal replaced the installed executable"
    );
    assert!(
        elapsed < BOOTSTRAP_PROBE_BOUND,
        "foreign resident refusal waited beyond its identity probe: {elapsed:?}"
    );
}

#[test]
fn restart_consumes_the_daemon_launchers_typed_readiness_without_a_second_probe() {
    let fixture = Fixture::new("restart-contract");
    let repository = fixture.repository();
    let epoch = "00000000-0000-4000-8000-000000000004"
        .parse()
        .expect("fixture epoch is canonical");
    let endpoint = "http://127.0.0.1:43821";
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: endpoint.to_owned(),
            epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let executable = fixture.binary(
        "spice-up-contract",
        format!("#!/bin/sh\nprintf '%s\\n' '{readiness}'\n").as_bytes(),
    );
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    let restarted = restart(
        &repository,
        &executable,
        (Some("127.0.0.1"), Some(43821)),
        &request,
        &mut output,
    );

    assert_eq!(
        restarted.expect("typed launcher readiness is sufficient"),
        Some(endpoint.to_owned())
    );
    assert!(
        String::from_utf8_lossy(&output).contains("resident ready at http://127.0.0.1:43821"),
        "refresh did not surface the admitted endpoint"
    );
}

#[test]
fn restart_surfaces_the_daemon_launchers_exact_refusal() {
    let fixture = Fixture::new("restart-refusal");
    let repository = fixture.repository();
    let executable = fixture.binary(
        "spice-up-refuses",
        b"#!/bin/sh\necho 'spice: resident::already-up; pid=42' >&2\nexit 2\n",
    );
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    let restarted = restart(
        &repository,
        &executable,
        (None, None),
        &request,
        &mut output,
    );

    assert!(
        matches!(restarted, Err(RefreshError::ResidentStartRefused(ref refusal))
            if refusal.contains("resident::already-up; pid=42")),
        "refresh replaced the launcher's exact refusal: {restarted:?}"
    );
}

/// A concurrent launcher can finish the install's job before its own `up`.
///
/// Treating the resulting `already-up` as a start failure enters rollback even
/// though the installed executable is serving. The bootstrap binary identity
/// makes that state exact rather than accepting an arbitrary lock holder.
#[test]
fn a_concurrent_installed_resident_completes_the_restart() {
    let fixture = Fixture::new("restart-current-race");
    let repository = fixture.repository();
    let installed = fixture.binary(
        "spice-up-refuses",
        b"#!/bin/sh\necho 'spice: resident::already-up; pid=42' >&2\nexit 2\n",
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let address = listener.local_addr().expect("bootstrap address reads");
    let endpoint = format!("http://{address}");
    let discovery = fixture.publish(&endpoint);
    let binary = executable_digest(&installed).expect("installed executable hashes");
    let server = serve_bootstrap(listener, discovery, binary);
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    let restarted = restart_admitting_reappearance(
        &repository,
        &installed,
        (Some("127.0.0.1"), Some(address.port())),
        &request,
        &mut output,
    );
    server.join().expect("bootstrap server joins");

    assert_eq!(
        restarted.expect("the installed resident is already ready"),
        Some(endpoint)
    );
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("resident already ready")
            && !output.contains("previous resident reappeared"),
        "the installed resident was not admitted directly: {output}"
    );
}

/// A predecessor that wins the dark interval is stopped before one bounded retry.
#[test]
fn a_concurrent_predecessor_is_replaced_by_the_installed_resident() {
    let fixture = Fixture::new("restart-predecessor-race");
    let repository = fixture.repository();
    let state = repository.admit_state().expect("fixture state is admitted");
    let marker = fixture.0.join("restart-attempted");
    let endpoint = "http://127.0.0.1:43822";
    let epoch = "00000000-0000-4000-8000-000000000004"
        .parse()
        .expect("fixture epoch is canonical");
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: endpoint.to_owned(),
            epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = down ]; then rm -f '{}'; exit 0; fi\nif [ -f '{}' ]; then printf '%s\\n' '{}'; exit 0; fi\n: > '{}'\necho 'spice: resident::already-up; pid=42' >&2\nexit 2\n",
        state.canvas().display(),
        marker.display(),
        readiness,
        marker.display(),
    );
    let installed = fixture.binary("spice-racing-up", script.as_bytes());
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let address = listener.local_addr().expect("bootstrap address reads");
    let published = format!("http://{address}");
    let discovery = fixture.publish(&published);
    let predecessor = Digest::compute(wire::Domain::DistArtifact, b"predecessor executable");
    let server = serve_bootstrap(listener, discovery, predecessor);
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    let restarted = restart_admitting_reappearance(
        &repository,
        &installed,
        (Some("127.0.0.1"), Some(43822)),
        &request,
        &mut output,
    );
    server.join().expect("bootstrap server joins");

    assert_eq!(
        restarted.expect("the predecessor is replaced"),
        Some(endpoint.to_owned())
    );
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("previous resident reappeared during install")
            && output.contains("stopping resident")
            && output.contains("resident ready at"),
        "the predecessor was not stopped and retried: {output}"
    );
}

#[test]
fn restart_forwards_up_arguments_and_does_not_duplicate_their_endpoint_options() {
    let request = RefreshRequest {
        restart: true,
        up_arguments: ["--host", "0.0.0.0", "--allow-insecure-bind"]
            .map(OsString::from)
            .into(),
        ..RefreshRequest::default()
    };

    let command = restart_command(
        Path::new("/installed/spice"),
        (Some("127.0.0.1"), Some(8767)),
        &request,
    );
    let arguments = command.get_args().collect::<Vec<_>>();

    assert_eq!(
        arguments,
        [
            "--json",
            "up",
            "--port",
            "8767",
            "--host",
            "0.0.0.0",
            "--allow-insecure-bind",
        ]
        .map(OsStr::new)
    );
}

#[test]
fn joined_forwarded_endpoint_options_also_override_published_values() {
    let request = RefreshRequest {
        restart: true,
        up_arguments: ["--host=0.0.0.0", "--port=9000"].map(OsString::from).into(),
        ..RefreshRequest::default()
    };

    let command = restart_command(
        Path::new("/installed/spice"),
        (Some("127.0.0.1"), Some(8767)),
        &request,
    );
    let arguments = command.get_args().collect::<Vec<_>>();

    assert_eq!(
        arguments,
        ["--json", "up", "--host=0.0.0.0", "--port=9000"].map(OsStr::new)
    );
}

#[test]
fn a_refresh_started_inside_a_lane_stops_the_resident_as_the_operator() {
    let command = resident_stop_command(Path::new("/installed/spice"));
    let lane = OsStr::new(wire::SpiceVariable::Lane.as_str());

    assert_eq!(
        command.get_envs().find(|(name, _)| *name == lane),
        Some((lane, None)),
        "the down child can inherit the lane principal"
    );
}

/// A clean stop is not reported as a refusal.
///
/// `down` succeeding is the outcome that has nothing to say, so the assertion
/// is that the refusal report stays absent: a status classified only as
/// "not success" would announce a refusal here that never happened.
#[test]
fn a_clean_stop_withdraws_without_reporting_a_refusal() {
    let fixture = Fixture::new("stop-clean");
    let repository = fixture.repository();
    let installed = fixture.binary("spice-down-clean", b"#!/bin/sh\nexit 0\n");
    fixture.publish(&format!("http://127.0.0.1:{}", unbound_port()));

    let mut output = Vec::new();
    let stopped = stop(&repository, &installed, &mut output);
    let output = String::from_utf8_lossy(&output).into_owned();

    assert!(stopped.is_ok(), "a clean stop was refused: {stopped:?}");
    assert!(
        output.contains("stopping resident"),
        "the stop was never announced: {output}"
    );
    assert!(
        !output.contains("down refused"),
        "a clean stop was reported as a refusal: {output}"
    );
}

/// A resident already gone refuses the stop, and the refresh proceeds anyway.
///
/// The refusal text is asserted on this command's own stream, which is where an
/// inherited stream would not have put it: `stop` captures the child's
/// diagnostics, so a version that let them through to the parent would leave
/// this buffer without them and still stop cleanly.
#[test]
fn a_refused_stop_with_a_dead_endpoint_proceeds_and_republishes_the_refusal() {
    let fixture = Fixture::new("stop-refused-dead");
    let repository = fixture.repository();
    let installed = fixture.binary(
        "spice-down-absent",
        b"#!/bin/sh\necho 'resident::not-running nothing to stop' >&2\nexit 7\n",
    );
    fixture.publish(&format!("http://127.0.0.1:{}", unbound_port()));

    let mut output = Vec::new();
    let stopped = stop(&repository, &installed, &mut output);
    let output = String::from_utf8_lossy(&output).into_owned();

    assert!(
        stopped.is_ok(),
        "a resident that was already gone failed the refresh: {stopped:?}"
    );
    assert!(
        output.contains("resident::not-running nothing to stop"),
        "the refusal text never reached the refresh's own stream: {output}"
    );
    assert!(
        output.contains("nothing answers the endpoint; continuing"),
        "proceeding past a dead endpoint was never reported: {output}"
    );
}

/// A refused stop with the endpoint still live is named, not waited out.
///
/// The elapsed bound is the half of this that the horizon would have satisfied
/// on its own: falling through to the withdrawal wait also ends in an error,
/// just fifteen seconds later and under a name that says the resident did not
/// stop rather than that `down` refused.
#[test]
fn a_refused_stop_with_a_live_endpoint_is_named_rather_than_waited_out() {
    let fixture = Fixture::new("stop-refused-live");
    let repository = fixture.repository();
    let installed = fixture.binary(
        "spice-down-refuses",
        b"#!/bin/sh\necho 'resident::down-refused still serving' >&2\nexit 7\n",
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a scratch port binds");
    let bound = listener.local_addr().expect("the scratch port is named");
    fixture.publish(&format!("http://127.0.0.1:{}", bound.port()));

    let mut output = Vec::new();
    let started = Instant::now();
    let stopped = stop(&repository, &installed, &mut output);
    let elapsed = started.elapsed();
    drop(listener);

    assert!(
        matches!(&stopped, Err(RefreshError::ResidentStopRefused(refusal))
            if refusal.contains("resident::down-refused still serving")),
        "a refused stop over a live endpoint was not named: {stopped:?}"
    );
    assert!(
        elapsed < Duration::from_millis(7_500),
        "the refusal fell through to the shutdown horizon: {elapsed:?}"
    );
}

/// The refresh `spice dev xtask -- install` performs, and the withdrawal it must not.
///
/// A rebuilt binary over a completed receipt derives a refresh rather than
/// refusing, so this command has nothing to withdraw first: it applies the very
/// plan a bare preview would show. Reading the installed executable throughout
/// proves the difference is observable rather than merely structural, because
/// reversing first would leave that path absent for a whole transaction.
#[test]
fn the_one_command_refresh_applies_a_bare_plan_without_withdrawing_anything() {
    let fixture = Fixture::new("rebuild");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    // Large enough that a withdrawal would be wide open to the reader below,
    // rather than a race it could miss between two polls.
    let first_image = vec![0x5a_u8; 512 * 1024];
    let second_image = vec![0xa5_u8; 512 * 1024];
    let first = fixture.binary("spice-first", &first_image);
    let second = fixture.binary("spice-second", &second_image);

    let first_digest = refresh_install(&repository, &first).expect("first install applies");
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        first_image
    );

    let planned = install::plan(&repository, Some(&second))
        .expect("a rebuilt binary plans a refresh rather than refusing");
    let previewed = planned.digest().expect("the refresh plan is canonical");

    let stop = Arc::new(AtomicBool::new(false));
    let first_seen = Arc::new(AtomicBool::new(false));
    let second_seen = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        let first_seen = Arc::clone(&first_seen);
        let second_seen = Arc::clone(&second_seen);
        let installed = installed.clone();
        let first_image = first_image.clone();
        let second_image = second_image.clone();
        thread::spawn(move || {
            let mut absences = 0_usize;
            let mut images = BTreeSet::new();
            while !stop.load(Ordering::Relaxed) {
                match fs::read(&installed) {
                    Ok(bytes) => {
                        first_seen.fetch_or(bytes == first_image, Ordering::Relaxed);
                        second_seen.fetch_or(bytes == second_image, Ordering::Relaxed);
                        images.insert(bytes);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => absences += 1,
                    Err(error) => panic!("the installed executable stopped reading: {error}"),
                }
            }
            (absences, images)
        })
    };
    wait_until_image_is_observed(&first_seen, "first installed image");
    let second_digest = refresh_install(&repository, &second).expect("refresh applies");
    wait_until_image_is_observed(&second_seen, "refreshed installed image");
    stop.store(true, Ordering::Relaxed);
    let (absences, images) = reader.join().expect("the reading thread joins");

    assert_eq!(
        absences, 0,
        "the installed executable was absent during a refresh"
    );
    // Without this the absence count is vacuous: a reader that ran entirely
    // before or after the swap would report the same nothing.
    assert!(
        images.contains(&first_image) && images.contains(&second_image),
        "the reader never straddled the swap, so it never had a withdrawal to miss"
    );
    assert_eq!(
        second_digest, previewed,
        "the command applied a plan a bare preview would not have shown"
    );
    assert_ne!(
        first_digest, second_digest,
        "a refreshed plan reused the previous identity"
    );
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        second_image
    );
}

fn wait_until_image_is_observed(observed: &AtomicBool, label: &str) {
    let started = Instant::now();
    while !observed.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(observed.load(Ordering::Relaxed), "reader missed {label}");
}

/// A receipt refusal before the installer writes anything is already a safe
/// terminal result. Feeding it through rollback only repeats the same refusal
/// and hides the useful cause in `RestoreFailed`.
#[test]
fn an_untouched_install_refusal_does_not_enter_rollback() {
    let fixture = Fixture::new("untouched-refusal");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let first = fixture.binary("spice-first", b"first build\n");
    let second = fixture.binary("spice-second", b"second build\n");

    refresh_install(&repository, &first).expect("first install applies");
    let drifted = repository
        .common_dir()
        .join(".spice-install/hooks/entry.sh");
    fs::write(&drifted, b"operator hook replacement\n")
        .expect("a non-binary installed asset drifts");
    let parked = park(&repository, &installed)
        .expect("the installed executable is parked")
        .expect("an installed executable exists to park");
    let before = snapshot_install(&repository, &installed).expect("install state snapshots");
    let refusal = refresh_install(&repository, &second)
        .expect_err("non-binary drift refuses before publication");
    assert!(matches!(
        &refusal,
        InstallError::Drift { path, .. } if path == ".spice-install/hooks/entry.sh"
    ));

    let mut output = Vec::new();
    let reported = restore_if_changed(
        &repository,
        &Rollback {
            installed: &installed,
            executable: Some(&parked),
            resident_executable: None,
            published: None,
            restart: false,
        },
        &before,
        &mut output,
        refusal,
    );

    assert!(
        matches!(&reported, RefreshError::Install(InstallError::Drift { path, .. })
            if path == ".spice-install/hooks/entry.sh"),
        "the original refusal was replaced: {reported:?}"
    );
    assert_eq!(
        snapshot_install(&repository, &installed).expect("install state reads back"),
        before
    );
    let output = String::from_utf8_lossy(&output);
    assert!(
        !output.contains("restoring previous executable")
            && !output.contains("restarting previous resident"),
        "an untouched refusal entered rollback: {output}"
    );
}

/// The default developer refresh stops a published resident before install.
/// An unchanged install refusal skips executable rollback but must still
/// restore that service boundary before returning the original refusal.
#[test]
fn an_untouched_install_refusal_restarts_the_resident_it_stopped() {
    let fixture = Fixture::new("untouched-resident-refusal");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let first = fixture.binary("spice-first", b"#!/bin/sh\nexit 3\n");
    let second = fixture.binary("spice-second", b"second build\n");

    refresh_install(&repository, &first).expect("first install applies");
    fs::write(
        repository
            .common_dir()
            .join(".spice-install/hooks/entry.sh"),
        b"operator hook replacement\n",
    )
    .expect("a non-binary installed asset drifts");
    let parked = park(&repository, &installed)
        .expect("the installed executable is parked")
        .expect("an installed executable exists to park");
    let before = snapshot_install(&repository, &installed).expect("install state snapshots");
    let refusal = refresh_install(&repository, &second)
        .expect_err("non-binary drift refuses before publication");

    let mut output = Vec::new();
    let reported = restore_if_changed(
        &repository,
        &Rollback {
            installed: &installed,
            executable: Some(&parked),
            resident_executable: None,
            published: Some("http://127.0.0.1:8767"),
            restart: true,
        },
        &before,
        &mut output,
        refusal,
    );
    let output = String::from_utf8_lossy(&output);

    assert!(matches!(
        &reported,
        RefreshError::Install(InstallError::Drift { path, .. })
            if path == ".spice-install/hooks/entry.sh"
    ));
    assert_eq!(
        snapshot_install(&repository, &installed).expect("install state reads back"),
        before
    );
    assert!(
        !output.contains("restoring previous executable"),
        "an unchanged executable was reinstalled: {output}"
    );
    assert!(
        output.contains("restarting previous resident")
            && output.contains(
                "previous resident did not come back: resident start refused: no diagnostic"
            ),
        "the stopped resident was not restored: {output}"
    );
}

/// A refresh that fails after the executable moved puts the previous one back
/// and still reports the failure that forced it.
#[test]
fn a_failed_refresh_restores_the_parked_executable_and_reports_its_cause() {
    let fixture = Fixture::new("restore");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let first = fixture.binary("spice-first", b"first build\n");
    let second = fixture.binary("spice-second", b"second build\n");

    refresh_install(&repository, &first).expect("first install applies");
    let parked = park(&repository, &installed)
        .expect("the installed executable is parked")
        .expect("an installed executable exists to park");
    refresh_install(&repository, &second).expect("second install applies");
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"second build\n"
    );

    let mut output = Vec::new();
    let reported = restore(
        &repository,
        &Rollback {
            installed: &installed,
            executable: Some(&parked),
            resident_executable: None,
            published: None,
            restart: false,
        },
        &mut output,
        RefreshError::ResidentUnready,
    );
    assert!(
        matches!(reported, RefreshError::ResidentUnready),
        "a successful restore replaced the reported cause: {reported}"
    );
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"first build\n"
    );
    assert!(
        !String::from_utf8_lossy(&output).contains("restarting previous resident"),
        "a refresh that never stopped a resident started one anyway: {}",
        String::from_utf8_lossy(&output)
    );
}

/// A refresh that stopped a resident restores both the prior installed bytes
/// and the distinct command authority that had actually been serving them.
#[test]
fn a_failed_refresh_restarts_the_resident_it_stopped() {
    let fixture = Fixture::new("resident-restore");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let first = fixture.binary("spice-first", b"first build\n");
    let second = fixture.binary("spice-second", b"second build\n");
    let restarted = fixture.0.join("resident-restored");
    let endpoint = "http://127.0.0.1:8767";
    let epoch = "00000000-0000-4000-8000-000000000004"
        .parse()
        .expect("fixture epoch is canonical");
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: endpoint.to_owned(),
            epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let resident = fixture.binary(
        "spice-serving-predecessor",
        format!(
            "#!/bin/sh\n: > '{}'\nprintf '%s\\n' '{}'\n",
            restarted.display(),
            readiness,
        )
        .as_bytes(),
    );

    refresh_install(&repository, &first).expect("first install applies");
    let parked = park(&repository, &installed)
        .expect("the installed executable is parked")
        .expect("an installed executable exists to park");
    refresh_install(&repository, &second).expect("second install applies");

    let mut output = Vec::new();
    let reported = restore(
        &repository,
        &Rollback {
            installed: &installed,
            executable: Some(&parked),
            resident_executable: Some(&resident),
            published: Some(endpoint),
            restart: true,
        },
        &mut output,
        RefreshError::ResidentUnready,
    );
    let output = String::from_utf8_lossy(&output).into_owned();

    assert!(
        matches!(reported, RefreshError::ResidentUnready),
        "a successful restore replaced the reported cause: {reported}"
    );
    assert!(
        restarted.exists() && output.contains("resident ready at"),
        "the stopped resident authority was not restarted: {output}"
    );
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"first build\n"
    );
}

/// Nothing is parked when the appliance has no executable installed yet.
#[test]
fn a_first_install_parks_no_previous_executable() {
    let fixture = Fixture::new("unparked");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    assert_eq!(
        park(&repository, &installed).expect("parking answers"),
        None,
        "an absent executable was reported as parked"
    );
}

/// The built executable is read from Cargo's own event for the `spice` binary.
#[test]
fn the_built_executable_is_the_one_cargo_named_for_spice() {
    let events = concat!(
        r#"{"reason":"compiler-artifact","target":{"name":"wire"},"executable":null}"#,
        "\n",
        r#"{"reason":"compiler-artifact","target":{"name":"spice"},"executable":null}"#,
        "\n",
        r#"{"reason":"compiler-artifact","target":{"name":"xtask"},"executable":"/w/xtask"}"#,
        "\n",
        r#"{"reason":"compiler-artifact","target":{"name":"spice"},"executable":"/w/spice"}"#,
        "\n",
        r#"{"reason":"build-finished","success":true}"#,
        "\n",
    );
    assert_eq!(
        built_executable(events),
        Some(PathBuf::from("/w/spice")),
        "the spice executable was not read out of the build events"
    );
}

/// A build that names no `spice` executable is not silently accepted.
#[test]
fn a_build_naming_no_spice_executable_answers_nothing() {
    let events = concat!(
        r#"{"reason":"compiler-artifact","target":{"name":"xtask"},"executable":"/w/xtask"}"#,
        "\n",
        "not json at all\n",
        r#"{"reason":"build-finished","success":true}"#,
        "\n",
    );
    assert_eq!(built_executable(events), None);
}

/// Published origins split into the host and port `up` accepts.
#[test]
fn a_published_origin_splits_into_a_host_and_port() {
    assert_eq!(
        endpoint_parts("http://127.0.0.1:8767"),
        Some(("127.0.0.1".to_owned(), 8767))
    );
    assert_eq!(
        endpoint_parts("http://127.0.0.1:8767/canvas"),
        Some(("127.0.0.1".to_owned(), 8767))
    );
    assert_eq!(
        endpoint_parts("127.0.0.1:8767"),
        Some(("127.0.0.1".to_owned(), 8767))
    );
    assert_eq!(endpoint_parts("http://127.0.0.1"), None);
    assert_eq!(endpoint_parts("http://127.0.0.1:port"), None);
}

/// An explicit host or port overrides the published one, and either alone still
/// resolves the other from what the resident had published.
#[test]
fn an_explicit_endpoint_overrides_the_published_one() {
    let published = Some("http://127.0.0.1:8767");
    let default = RefreshRequest::default();
    assert_eq!(
        requested_endpoint(&default, published),
        (Some("127.0.0.1".to_owned()), Some(8767))
    );

    let ported = RefreshRequest {
        port: Some(8768),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, published),
        (Some("127.0.0.1".to_owned()), Some(8768))
    );

    let hosted = RefreshRequest {
        host: Some("0.0.0.0".to_owned()),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&hosted, published),
        (Some("0.0.0.0".to_owned()), Some(8767))
    );
}

/// With nothing published and nothing asked for, the resident is left to pick
/// its own endpoint rather than being pinned to a guess.
#[test]
fn an_unpublished_endpoint_with_no_override_is_left_unbound() {
    assert_eq!(
        requested_endpoint(&RefreshRequest::default(), None),
        (None, None)
    );
}

/// An explicitly named port survives having no published endpoint to pair with.
///
/// Requiring a host too would drop the operator's port and leave `up` on its own
/// default. The first refresh of a repository is exactly when nothing is
/// published yet, so the explicit half must survive on its own.
#[test]
fn an_explicit_endpoint_survives_an_unpublished_counterpart() {
    let ported = RefreshRequest {
        port: Some(8768),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, None),
        (None, Some(8768)),
        "an explicitly named port was dropped for want of a host"
    );

    let hosted = RefreshRequest {
        host: Some("127.0.0.1".to_owned()),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&hosted, None),
        (Some("127.0.0.1".to_owned()), None),
        "an explicitly named host was dropped for want of a port"
    );
}

/// Two fixtures sharing one label take separate roots and leave neither behind.
///
/// The scratch name used to carry the process id, and nothing removed the root
/// afterwards. Those two defects only bite together: ids are recycled, so a run
/// that inherited one found a root an earlier run had abandoned under exactly
/// the name it wanted, and `create_dir` refused before the test under it ran.
/// Holding both fixtures at once is what proves the second name is not the
/// first, and reading the paths back after the scope closes is what proves
/// neither root survives to be collided with.
#[test]
fn two_fixtures_sharing_a_label_take_separate_roots_and_leave_neither_behind() {
    let identifier = std::process::id().to_string();
    let (first, second) = {
        let earlier = Fixture::new("collision");
        let later = Fixture::new("collision");
        assert_ne!(earlier.0, later.0, "one label named one root twice");

        for root in [&earlier.0, &later.0] {
            assert!(
                root.is_dir(),
                "{} is not a live scratch root",
                root.display()
            );
            let name = root
                .file_name()
                .expect("a scratch root is named")
                .to_string_lossy()
                .into_owned();
            assert!(
                !name.split('-').any(|field| field == identifier),
                "{name} still names the process id the operating system reuses"
            );
        }
        (earlier.0.clone(), later.0.clone())
    };

    assert!(
        !first.exists(),
        "{} outlived the fixture that owns it",
        first.display()
    );
    assert!(
        !second.exists(),
        "{} outlived the fixture that owns it",
        second.display()
    );
}
