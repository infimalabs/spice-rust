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
        Some(&parked),
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
        Some(("127.0.0.1".to_owned(), 8765))
    );

    let ported = RefreshRequest {
        port: Some(8767),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, published),
        Some(("127.0.0.1".to_owned(), 8767))
    );

    let hosted = RefreshRequest {
        host: Some("0.0.0.0".to_owned()),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&hosted, published),
        Some(("0.0.0.0".to_owned(), 8765))
    );
}

/// With nothing published and nothing asked for, the resident is left to pick
/// its own endpoint rather than being pinned to a guess.
#[test]
fn an_unpublished_endpoint_with_no_override_is_left_unbound() {
    assert_eq!(requested_endpoint(&RefreshRequest::default(), None), None);

    let ported = RefreshRequest {
        port: Some(8767),
        ..RefreshRequest::default()
    };
    assert_eq!(
        requested_endpoint(&ported, None),
        None,
        "a port with no host bound an invented host"
    );
}
