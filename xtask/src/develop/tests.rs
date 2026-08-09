use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// A scratch repository the installer is allowed to publish into.
struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-develop-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("exclusive scratch root is created");
        let root = fs::canonicalize(&root).expect("scratch root canonicalizes");
        let status = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main", "."])
            .current_dir(&root)
            .status()
            .expect("git launches");
        assert!(status.success(), "scratch repository is initialized");
        Self(root)
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

    /// Publish a discovery naming `endpoint`, the way a resident would.
    fn publish(&self, endpoint: &str) {
        let identity = "00000000-0000-4000-8000-000000000000"
            .parse::<wire::CanonicalUuid>()
            .expect("the fixture identity is canonical");
        let discovery = canvas::Discovery {
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
    }
}

/// A process that stays alive until the test drops it.
fn standing_child() -> std::process::Child {
    Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("a standing child launches")
}

/// Readiness is the published endpoint answering, not the record existing.
///
/// The record is written before anything binds, so a readiness check that
/// trusted it would pass here against a port nothing is listening on.
#[test]
fn a_published_endpoint_is_ready_only_once_it_answers() {
    let fixture = Fixture::new("ready");
    let repository = fixture.repository();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a scratch port binds");
    let bound = listener.local_addr().expect("the scratch port is named");
    let mut child = standing_child();

    fixture.publish(&format!("http://127.0.0.1:{}", bound.port()));
    let answered = ready(&repository, &mut child, Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        answered.expect("a listening endpoint is ready"),
        format!("http://127.0.0.1:{}", bound.port())
    );

    drop(listener);
    let mut child = standing_child();
    let closed = ready(&repository, &mut child, Duration::from_millis(300));
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        matches!(closed, Err(RefreshError::ResidentUnready)),
        "the same record was ready with nothing listening: {closed:?}"
    );
}

/// A failed start reports the resident's own last diagnostic, not just a status.
///
/// `canvas::bind-failed` and its kin are written only to the resident's log, so
/// a refresh that printed the exit status alone would name a number whose reason
/// it never surfaced.
#[test]
fn a_failed_start_reports_the_residents_own_last_diagnostic() {
    let fixture = Fixture::new("residentlog");
    let repository = fixture.repository();
    let log = workspace(&repository)
        .expect("the scratch workspace is created")
        .join(RESIDENT_LOG);

    let mut empty = Vec::new();
    report_resident_log(&repository, &mut empty);
    assert!(
        String::from_utf8_lossy(&empty).contains("recorded nothing"),
        "an absent resident log was reported as a diagnostic: {}",
        String::from_utf8_lossy(&empty)
    );

    fs::write(
        &log,
        "starting\nspice: canvas::bind-failed; listening on 127.0.0.1:8768 failed\n\n",
    )
    .expect("the scratch resident log is written");
    let mut reported = Vec::new();
    report_resident_log(&repository, &mut reported);
    let reported = String::from_utf8_lossy(&reported).into_owned();
    assert!(
        reported.contains("canvas::bind-failed; listening on 127.0.0.1:8768 failed"),
        "the resident's own diagnostic was not surfaced: {reported}"
    );
    assert!(
        reported.contains(&log.display().to_string()),
        "the report did not name the log it read: {reported}"
    );
}

/// A resident that dies during startup is reported as an exit, not a timeout.
#[test]
fn a_resident_that_exits_before_readiness_is_reported_as_an_exit() {
    let fixture = Fixture::new("exited");
    let repository = fixture.repository();
    let mut child = Command::new("sh")
        .args(["-c", "exit 9"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("a failing child launches");

    let exited = ready(&repository, &mut child, Duration::from_secs(5));
    assert!(
        matches!(exited, Err(RefreshError::ResidentExited(Some(9)))),
        "an early resident exit was not reported with its status: {exited:?}"
    );
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
        elapsed < SHUTDOWN_HORIZON,
        "the refusal fell through to the shutdown horizon: {elapsed:?}"
    );
}

/// The refusal `cargo xtask install` exists to answer, and the refresh that
/// answers it.
///
/// A completed receipt pins the executable bytes it installed, so the plan
/// behind `spice install` and `spice install --apply` cannot be derived at all
/// once the tree is rebuilt. The bare plan is asserted to refuse first, because
/// a refresh that merely worked would not show that anything was wrong.
#[test]
fn a_rebuilt_binary_refuses_a_bare_plan_and_refreshes_through_one_command() {
    let fixture = Fixture::new("rebuild");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    let first = fixture.binary("spice-first", b"first build\n");
    let second = fixture.binary("spice-second", b"second build\n");

    let first_digest = refresh_install(&repository, &first).expect("first install applies");
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"first build\n"
    );

    let refused = install::plan(&repository, Some(&second));
    assert!(
        matches!(refused, Err(InstallError::BinaryContextChanged)),
        "a bare plan admitted a rebuilt binary: {refused:?}"
    );

    let second_digest = refresh_install(&repository, &second).expect("refresh applies");
    assert_ne!(
        first_digest, second_digest,
        "a refreshed plan reused the previous identity"
    );
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"second build\n"
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

/// A refresh that stopped a resident brings it back, and a resident that will
/// not come back still does not replace the cause that forced the restore.
///
/// Restoring the executable alone would leave the appliance installed but
/// unserved, which is not the state the refresh was asked to preserve.
#[test]
fn a_failed_refresh_restarts_the_resident_it_stopped() {
    let fixture = Fixture::new("resident-restore");
    let repository = fixture.repository();
    let installed = install::installed_binary(&repository);
    // The restored executable is what the restart is launched from, so a script
    // that refuses to stay up is a resident that cannot come back.
    let first = fixture.binary("spice-first", b"#!/bin/sh\nexit 3\n");
    let second = fixture.binary("spice-second", b"second build\n");

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
            published: Some("http://127.0.0.1:8767"),
            restart: true,
        },
        &mut output,
        RefreshError::ResidentUnready,
    );
    let output = String::from_utf8_lossy(&output).into_owned();

    assert!(
        matches!(reported, RefreshError::ResidentUnready),
        "the restore's own aftermath replaced the reported cause: {reported}"
    );
    assert!(
        output.contains("restarting previous resident"),
        "a stopped resident was never restarted: {output}"
    );
    assert!(
        output.contains("previous resident did not come back: resident exited with status 3"),
        "a resident that refused to come back was not reported: {output}"
    );
    assert_eq!(
        fs::read(&installed).expect("installed executable reads"),
        b"#!/bin/sh\nexit 3\n"
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
    let published = Some("http://127.0.0.1:8765");
    let default = RefreshRequest::default();
    assert_eq!(
        requested_endpoint(&default, published),
        (Some("127.0.0.1".to_owned()), Some(8765))
    );

    let ported = RefreshRequest {
        port: Some(8767),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, published),
        (Some("127.0.0.1".to_owned()), Some(8767))
    );

    let hosted = RefreshRequest {
        host: Some("0.0.0.0".to_owned()),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&hosted, published),
        (Some("0.0.0.0".to_owned()), Some(8765))
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
/// default, which is the endpoint this repository reserves for the predecessor:
/// the first refresh of a repository is exactly when nothing is published yet.
#[test]
fn an_explicit_endpoint_survives_an_unpublished_counterpart() {
    let ported = RefreshRequest {
        port: Some(8767),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, None),
        (None, Some(8767)),
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
