//! Runtime and shutdown regressions for the split resident surface.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant as Monotonic};

use sill::{
    Argv, Cancellation, Deadlines, Environment, ObservationFailure, Output, OutputStream,
    ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn fixture(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-resident-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn listen_for(argv: &[&str]) -> CommandResult<SocketAddr> {
    let matches = spice::cli::command()
        .try_get_matches_from(argv)
        .expect("declared up operands parse");
    requested_listen(spice::cli::selected_matches(&matches))
}

fn listen_refusal_for(argv: &[&str]) -> String {
    match listen_for(argv) {
        Ok(listen) => panic!("{argv:?} must not bind {listen}"),
        Err(Outcome::Refusal(refusal)) => refusal.reason,
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_for(argv: &[&str]) -> CommandResult<canvas::axum::Access> {
    let matches = spice::cli::command()
        .try_get_matches_from(argv)
        .expect("declared up operands parse");
    let leaf = spice::cli::selected_matches(&matches);
    let listen = requested_listen(leaf)?;
    requested_access(leaf, listen)
}

fn access_refusal(argv: &[&str]) -> Refusal {
    match access_for(argv) {
        Ok(access) => panic!("{argv:?} must not admit {access:?}"),
        Err(Outcome::Refusal(refusal)) => refusal,
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_refusal_for(argv: &[&str]) -> String {
    access_refusal(argv).reason
}

/// The surface listens on exactly what the operator named, or on one fixed
/// default it can be told about in advance.
///
/// Both halves are load-bearing. An operator reaching this surface from another
/// machine has to be able to say which address it answers on, and an operator
/// who says nothing has to get the same URL back after a restart: an ephemeral
/// port would hand them a new address every time, which no bookmark, open tab,
/// or written-down instruction survives.
#[test]
fn the_listen_address_is_what_was_asked_for_or_one_stable_default() {
    assert_eq!(listen_for(&["spice", "up"]), Ok(DEFAULT_LISTEN));
    assert_ne!(
        DEFAULT_LISTEN.port(),
        0,
        "an ephemeral default port changes the URL on every restart"
    );

    assert_eq!(
        listen_for(&["spice", "up", "--host", "0.0.0.0"]),
        Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            DEFAULT_LISTEN.port()
        )),
        "a named host keeps the default port"
    );
    assert_eq!(
        listen_for(&["spice", "up", "--port", "9000"]),
        Ok(SocketAddr::new(DEFAULT_LISTEN.ip(), 9000)),
        "a named port keeps the default host"
    );
    assert_eq!(
        listen_for(&["spice", "up", "--host", "10.0.0.5", "--port", "9000"]),
        Ok("10.0.0.5:9000".parse().expect("fixture address")),
        "both named is bound verbatim"
    );
}

/// An address this machine cannot listen on is named rather than approximated.
///
/// Resolving a host name here would bind whatever the resolver answered with,
/// which is not what the operator asked for and not something they could check
/// afterwards, so a name is refused instead of guessed at.
#[test]
fn an_unbindable_listen_request_is_refused_by_name() {
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--host", "canvas.example"]),
        "resident::host-invalid"
    );
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--port", "70000"]),
        "resident::port-invalid"
    );
    assert_eq!(
        listen_refusal_for(&["spice", "up", "--port", "http"]),
        "resident::port-invalid"
    );
}

#[test]
fn remote_listening_requires_explicit_consent_and_a_url_safe_token() {
    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "0.0.0.0"]),
        "resident::insecure-bind-consent-required"
    );
    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "0.0.0.0", "--allow-insecure-bind",]),
        "resident::auth-token-required"
    );
    let invalid = access_refusal(&[
        "spice",
        "up",
        "--host",
        "0.0.0.0",
        "--allow-insecure-bind",
        "--auth-token",
        "too-short-token",
    ]);
    assert_eq!(invalid.reason, "resident::auth-token-invalid");
    assert_eq!(
        invalid.observed, None,
        "a refused credential is never echoed"
    );
    assert!(
        access_for(&[
            "spice",
            "up",
            "--host",
            "0.0.0.0",
            "--allow-insecure-bind",
            "--auth-token",
            "correct-horse-battery-staple",
        ])
        .is_ok(),
        "explicit consent and a caller-held token admit the remote bind"
    );
    assert!(
        access_for(&["spice", "up"]).is_ok(),
        "the stable loopback default remains frictionless"
    );
}

/// `::1` is loopback here and at the listener, because one predicate says so.
///
/// This gate and the canvas one both ask [`std::net::IpAddr::is_loopback`]. A
/// narrowing on either side would split one decision in two: this side would
/// admit `--host ::1` with no token, and the listener would refuse the address
/// it had just admitted, telling the operator a machine-local bind is remote.
///
/// The wildcard cases sit beside the loopback one deliberately. An `is_ok` on
/// `::1` alone passes just as well against a gate that admits everything, and
/// `::` is the address that a rule reading only `Ipv4Addr` would wave through.
#[test]
fn ipv6_loopback_needs_no_consent_or_token_while_its_wildcard_needs_both() {
    assert!(
        access_for(&["spice", "up", "--host", "::1"]).is_ok(),
        "IPv6 loopback is reachable from this machine alone"
    );

    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "::"]),
        "resident::insecure-bind-consent-required"
    );
    assert_eq!(
        access_refusal_for(&["spice", "up", "--host", "::", "--allow-insecure-bind"]),
        "resident::auth-token-required"
    );
}

#[test]
fn runtime_paths_follow_the_repository_common_directory() {
    let root = fixture("main");
    let nested = root.join("a/b");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    fs::create_dir_all(&nested).expect("nested directory");

    let paths = RuntimePaths::discover(&nested).expect("paths resolve");
    let common = fs::canonicalize(root.join(".git")).expect("canonical common directory");

    assert_eq!(paths.runtime, common.join(".spice"));
    assert_eq!(
        paths.journal,
        common.join(".spice/data/spicejournal.sqlite3")
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

/// A holder replacement never creates an empty observation for a contender.
///
/// The callback is the exact instant after the writer has prepared its one
/// landing write and before that write occurs. The second acquisition therefore
/// exercises the resident's real contended-refusal path while the replacement
/// is in flight. Restoring a truncation before that callback makes this fixture
/// observe `holder metadata unavailable` instead of the seeded holder.
#[test]
fn contended_resident_names_the_holder_while_its_record_is_rewritten() {
    let root = fixture("holder-rewrite");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.runtime).expect("runtime directory");
    let lock = LockFile::new(
        LockName::new("resident").expect("resident lock name"),
        &paths.resident,
    );
    let Acquisition::Acquired(possession) = lock.try_acquire().expect("holder acquires") else {
        panic!("fresh resident lock unexpectedly contended");
    };
    let seeded = "pid=4242;epoch=seeded-holder";
    possession
        .rewrite_record(seeded.as_bytes())
        .expect("seeded holder lands");

    let mut observed = None;
    possession
        .rewrite_record_with(b"pid=4242;epoch=replacement", || {
            let Err(Outcome::Refusal(refusal)) = ResidentLock::acquire(&paths) else {
                panic!("the resident contender did not refuse");
            };
            assert_eq!(refusal.reason, "resident::already-up");
            observed = refusal.observed;
            Ok(())
        })
        .expect("replacement holder lands");

    assert_eq!(observed.as_deref(), Some(seeded));
    assert_eq!(
        fs::read_to_string(&paths.resident)
            .expect("replacement holder reads")
            .trim(),
        "pid=4242;epoch=replacement"
    );
    drop(possession);
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn stale_surface_cleanup_is_idempotent_and_leaves_inbox_authority() {
    let root = fixture("stale-surfaces");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("paths resolve");
    fs::create_dir_all(&paths.lane_canvas).expect("lane discovery directory");
    fs::write(&paths.canvas, b"stale operator discovery").expect("operator discovery");
    fs::write(paths.lane_canvas.join("lane"), b"stale lane discovery").expect("lane discovery");
    fs::create_dir_all(&paths.inbox).expect("inbox directory");
    fs::write(paths.inbox.join("lane"), b"durable pending direction").expect("lane inbox");

    paths.clear_stale_surfaces().expect("stale surfaces clear");
    paths
        .clear_stale_surfaces()
        .expect("repeated stale surface cleanup is ordinary");

    assert!(!paths.canvas.exists());
    assert!(!paths.lane_canvas.exists());
    assert_eq!(
        fs::read(paths.inbox.join("lane")).expect("inbox remains"),
        b"durable pending direction"
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn linked_worktrees_share_the_git_common_directory() {
    let root = fixture("linked");
    let worktree = root.join("worktree");
    let common = root.join("common.git");
    let git_dir = common.join("worktrees/linked");
    fs::create_dir_all(&worktree).expect("worktree directory");
    fs::create_dir_all(&git_dir).expect("git directory");
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .expect("git marker");
    fs::write(git_dir.join("commondir"), "../..\n").expect("common marker");

    let paths = RuntimePaths::discover(&worktree).expect("paths resolve");
    let common = fs::canonicalize(&common).expect("canonical common directory");

    assert_eq!(paths.runtime, common.join(".spice"));
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn startup_surface_captures_configured_hidden_project_policy_once() {
    let root = fixture("hidden-project-policy");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    fs::write(
        root.join("spice.toml"),
        "[tasks]\nstems = ['board']\nhidden_stems = ['sandbox']\n",
    )
    .expect("tracked task metadata");
    let repository = Repository::discover(&root).expect("fixture repository resolves");

    let startup = load_startup_surface(&repository).expect("startup policy resolves");
    assert_eq!(
        startup.config.projects.hidden_stems(),
        ["oops", "maxim_proposal", "sandbox"]
    );
    assert_eq!(
        startup
            .config
            .projects
            .admit(".sandbox.triage_1")
            .expect("configured hidden descendant")
            .visibility()
            .hidden_stem(),
        Some("sandbox")
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn cooperative_and_forced_groups_drive_observed_counts_and_residue() {
    let (root, paths) = resident_fixture("groups");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let mut seats = Seats::new();

    let drained_lane = test_uuid(2);
    let drained_occupant = test_uuid(3);
    seats
        .seat(
            drained_lane,
            drained_occupant,
            process_spec("sleep 0.05", Duration::from_secs(5)),
        )
        .expect("cooperative group starts");
    seed_lane(&bus, epoch, drained_lane, drained_occupant);

    let killed_lane = test_uuid(4);
    let killed_occupant = test_uuid(5);
    seats
        .seat(
            killed_lane,
            killed_occupant,
            process_spec(
                "trap '' TERM; while :; do sleep 1; done",
                Duration::from_millis(100),
            ),
        )
        .expect("non-cooperative group starts");
    seed_lane(&bus, epoch, killed_lane, killed_occupant);

    fs::create_dir_all(&paths.degraded).expect("degraded directory");
    fs::write(
        paths.degraded.join(drained_lane.to_string()),
        "offline run\n",
    )
    .expect("degraded marker");
    fs::create_dir_all(&paths.tape).expect("tape directory");
    let tape_lane = test_uuid(6);
    fs::write(paths.tape.join(tape_lane.to_string()), "not a tape")
        .expect("malformed tape residue");
    let sealed_lane = test_uuid(8);
    let tape = tape::Tape::open(paths.tape.join(sealed_lane.to_string()).join("claude"))
        .expect("live tape opens");
    let locus = tape
        .tee(b"resident shutdown evidence")
        .expect("tape captures");

    let shutdown_started = Monotonic::now();
    assert!(matches!(
        drive_shutdown(&bus, &paths, epoch, &mut seats),
        Outcome::Reply(_)
    ));
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(2),
        "one group's unused runtime extended another group's drain bound"
    );
    assert!(seats.is_empty());
    bus.verify().expect("Dark authority verifies");
    let snapshot = bus.fact_snapshot().expect("facts read through bus");
    assert_eq!(
        fact_u64(&snapshot, wire::FactKind::UpDraining, "occupants"),
        2
    );
    assert_eq!(
        fact_u64(&snapshot, wire::FactKind::UpTerminating, "occupants"),
        1
    );
    assert_eq!(
        fact_u64(&snapshot, wire::FactKind::UpKilling, "occupants"),
        1
    );
    assert!(
        snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::LaneDrained)
    );
    assert!(
        snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::LaneExited)
    );
    let degraded = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::LaneDegraded)
        .count();
    assert_eq!(degraded, 2, "disk residue was not named once per lane");
    let head = bus
        .residency_head()
        .expect("head read")
        .expect("head exists")
        .into_body();
    assert_eq!(head.state, ResidentState::Dark);
    assert_eq!(head.lanes.len(), 2);
    assert!(head.lanes.iter().all(|lane| lane.occupant.is_none()));
    assert!(!paths.degraded.join(drained_lane.to_string()).exists());
    assert_eq!(
        tape.read(locus).expect("sealed tape remains readable"),
        b"resident shutdown evidence"
    );
    assert!(matches!(
        tape.seal().expect("tape was sealed"),
        tape::Seal::Empty
    ));
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn a_post_seated_surface_failure_compensates_to_verified_dark() {
    let (root, paths) = resident_fixture("compensation");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(7);
    let startup = commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let original = environment_fault("canvas::fixture-failed", "after UpSeated");

    let settled = settle_surface(
        &bus,
        &paths,
        epoch,
        startup,
        &mut Seats::new(),
        Err(original),
    );

    let Outcome::Fault(fault) = settled else {
        panic!("the original startup failure must remain visible");
    };
    assert_eq!(fault.reason, "canvas::fixture-failed");
    let head = bus
        .residency_head()
        .expect("head read")
        .expect("head exists")
        .into_body();
    assert_eq!(head.state, ResidentState::Dark);
    assert!(head.lanes.is_empty());
    bus.verify().expect("compensated authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn capture_failure_vacates_and_durably_degrades_the_exact_occupant() {
    let (root, paths) = resident_fixture("capture-failure");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec("exec /usr/bin/tail -f /dev/null", Duration::from_secs(5)),
        )
        .expect("capture fixture group starts");
    seed_lane(&bus, epoch, lane, occupant);

    super::lane_surface::capture_failed(
        &bus,
        epoch,
        &mut seats,
        &lane::capture::CaptureFailure {
            lane,
            occupant,
            failure: ObservationFailure::SinkRefused {
                stream: OutputStream::Stdout,
                detail: "fixture tape refusal".to_owned(),
            },
        },
    )
    .expect("capture failure commits");

    assert!(!seats.occupied(lane));
    let head = current_residency(&bus, epoch)
        .expect("capture residency reads")
        .into_body();
    let row = head
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .expect("capture lane remains durable");
    assert_eq!((row.occupant, row.disposition.as_str()), (None, "degraded"));
    let snapshot = bus.fact_snapshot().expect("capture facts read");
    let degraded = snapshot
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == wire::FactKind::LaneDegraded)
        .expect("capture degradation is named");
    assert!(
        degraded.fact.body()["marker"]
            .as_str()
            .is_some_and(|marker| marker.contains("fixture tape refusal"))
    );
    bus.shutdown().expect("capture fixture bus stops");
    fs::remove_dir_all(root).expect("capture fixture cleanup");
}

fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
    let root = fixture(name);
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.data).expect("data directory");
    (root, paths)
}

fn process_spec(command: &str, runtime: Duration) -> Spec {
    Spec {
        argv: Argv::new("/bin/sh", ["-c", command]).expect("fixture argv"),
        cwd: PathBuf::from("/"),
        environment: Environment::new([("PATH", "/usr/bin:/bin")]).expect("fixture environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
            .expect("fixture deadlines"),
        wait: WaitPolicy::new(
            "resident fixture",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .expect("fixture wait policy"),
        output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
        redaction: Redaction::None,
        grace: Duration::from_millis(100),
    }
}

fn seed_lane(bus: &Bus, epoch: CanonicalUuid, lane: CanonicalUuid, occupant: CanonicalUuid) {
    let current = current_residency(bus, epoch).expect("current residency");
    let mut head = current.body().clone();
    head.lanes.push(wire::LaneResidency {
        lane,
        worktree: "/fixture".to_owned(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![test_uuid(9)],
        session: None,
        disposition: "occupied".to_owned(),
    });
    head.lanes.sort_by_key(|lane| lane.lane);
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                capability: test_uuid(9),
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane seats");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 123,
            },
            FactCause::Pending(seated),
        )
        .expect("occupant starts");
    write.residency_head(started, move |_| head);
    commit(bus, write).expect("fixture lane commits");
}

fn fact_u64(snapshot: &journal::FactSnapshot, kind: wire::FactKind, field: &str) -> u64 {
    snapshot
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == kind)
        .and_then(|fact| fact.fact.body().get(field))
        .and_then(|value| value.as_u64())
        .expect("fact carries numeric fixture field")
}

fn test_uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}

/// One published discovery, at a path the caller owns for the test.
fn published_discovery(name: &str, port: u16) -> (PathBuf, PathBuf, canvas::Discovery) {
    let root = fixture(name);
    fs::create_dir_all(&root).expect("announcement fixture root");
    let path = root.join("canvas");
    let discovery = canvas::Discovery {
        endpoint: format!("http://127.0.0.1:{port}"),
        capability: test_uuid(1),
        epoch: test_uuid(2),
    };
    discovery
        .publish(&path)
        .expect("fixture discovery publishes");
    (root, path, discovery)
}

/// The announced endpoint is one that passed admission, and nothing dials it.
///
/// This is the whole reason `up` reads its own discovery back rather than
/// formatting the address it bound: the operator is handed a URL only after
/// the same ownership, loopback-origin, and generation checks a client makes.
/// A plain start must not seize the operator's browser, so the launcher stays
/// untouched until `--open` asks for it.
#[test]
fn announcing_the_canvas_names_the_admitted_endpoint_without_dialling_it() {
    let (root, path, discovery) = published_discovery("announce-endpoint", 48231);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        false,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {}\n", discovery.endpoint)
    );
    assert!(
        diagnostics.is_empty(),
        "an admitted endpoint is not a diagnostic: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    assert_eq!(dialled, None, "a plain start must not open a browser");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// `--open` hands the launcher the admitted endpoint and no other.
#[test]
fn opening_at_start_dials_exactly_the_endpoint_that_was_announced() {
    let (root, path, discovery) = published_discovery("announce-open", 48232);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(dialled.as_deref(), Some(discovery.endpoint.as_str()));
    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {}\n", discovery.endpoint)
    );
    assert!(
        diagnostics.is_empty(),
        "an opened endpoint is not a diagnostic: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

/// A generation replaced under the announcement is refused, not announced.
///
/// Folding the opener into `up` only preserves the admission it replaced if
/// the resident's own read is subject to it. Publishing a new generation
/// between the read and the identity recheck is the exact race that check
/// exists for, and it must reach the announcement as a refusal rather than as
/// a URL belonging to the generation that just lost.
///
/// It must also reach a different stream. Since the endpoint moved to stdout so
/// a shell can capture it, anything else written there is read as an address,
/// and this is the one case that produces something else.
#[test]
fn announcing_the_canvas_refuses_a_generation_that_was_replaced() {
    let (root, path, discovery) = published_discovery("announce-stale", 48233);
    let replacement = canvas::Discovery {
        capability: test_uuid(3),
        epoch: test_uuid(4),
        ..discovery.clone()
    };
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        true,
        || {
            replacement
                .publish(&path)
                .expect("replacement publishes before the identity recheck");
        },
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    assert_eq!(
        String::from_utf8(diagnostics).expect("diagnostic is text"),
        "canvas unavailable: canvas::discovery-replaced\n"
    );
    assert!(
        announced.is_empty(),
        "a refusal reason is not an endpoint, and a shell reading the endpoint \
         stream would dial it: {}",
        String::from_utf8_lossy(&announced)
    );
    assert_eq!(dialled, None, "a stale generation must not be dialled");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}

#[test]
fn durable_capture_projects_only_prose_with_occupant_and_event_identity() {
    let bus = Bus::start(journal::Journal::in_memory().expect("message authority opens"))
        .expect("message bus starts");
    let source = canvas::stream::Source::new(bus.reader(), test_uuid(9), 8);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let locus = listen::Locus::new(4, 120, 48);
    let record = lane::capture::CaptureRecord {
        lane: test_uuid(5),
        occupant: test_uuid(6),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events(vec![
            listen::Event::Reasoning(listen::Reasoning {
                content: listen::ReasoningContent::Opaque,
                locus,
            }),
            listen::Event::Prose(listen::Prose {
                text: "agent-authored answer".to_owned(),
                phase: None,
                locus,
            }),
        ]),
    };

    publish_agent_messages(&publisher, &record).expect("capture projects");

    let message = updates.try_recv().expect("prose publishes once");
    assert_eq!(message.lane, record.lane);
    assert_eq!(message.author, record.occupant);
    assert_eq!(message.text, "agent-authored answer");
    assert_eq!(
        message.locus,
        wire::MessageLocus {
            member: 4,
            offset: 120,
            length: 48,
            event: 1,
        }
    );
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    drop(source);
    bus.shutdown().expect("message bus stops");
}

/// A generation that was never published is a diagnostic, not a silent start.
///
/// This is the operator's first start, before anything has been published, and
/// it reaches the announcement through `DiscoveryRead::open` rather than the
/// identity recheck the replaced-generation case above exercises.
///
/// Both arrive as *faults*: probing this path, the replaced path, an
/// undialable endpoint, and a failed launch produced `Outcome::Fault` every
/// time, so `announce_canvas`'s `Outcome::Refusal` arm is held by nothing here.
/// Rerouting that one arm to the endpoint stream reds no test in this file.
/// Recorded rather than guessed at: it is not clear the arm is reachable at
/// all, and a fixture that cannot fail would be worse than this note.
#[test]
fn announcing_the_canvas_without_a_generation_names_no_endpoint() {
    let root = fixture("announce-missing");
    fs::create_dir_all(&root).expect("announcement fixture root");
    let path = root.join("canvas");
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    let diagnostic = String::from_utf8(diagnostics).expect("diagnostic is text");
    assert!(
        diagnostic.starts_with("canvas unavailable: "),
        "a missing generation must be reported: {diagnostic}"
    );
    assert!(
        announced.is_empty(),
        "a shell capturing the endpoint stream would dial this: {}",
        String::from_utf8_lossy(&announced)
    );
    assert_eq!(dialled, None, "a missing generation must not be dialled");
    fs::remove_dir_all(&root).expect("announcement fixture cleanup");
}
