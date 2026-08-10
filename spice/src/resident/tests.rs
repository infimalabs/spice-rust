//! Runtime and shutdown regressions for the split resident surface.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant as Monotonic};

use sill::{
    Argv, Cancellation, Deadlines, Environment, Observation, ObservationFailure, ObserveError,
    ObservedRecord, Output, OutputStream, ProcessGroup, RecordObserver, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};

use super::*;

// Canvas announcement and capture fixtures live behind one named boundary so
// this resident root stays below the file-shape base, not merely its ratchet.
#[path = "tests/canvas_announcement.rs"]
mod canvas_announcement;
#[path = "tests/media.rs"]
mod media;
#[path = "tests/message_unavailable.rs"]
mod message_unavailable;
#[path = "tests/startup_window.rs"]
mod startup_window;

use canvas_announcement::{
    fact_u64, process_spec, resident_fixture, seed_lane, seed_lane_in_lineage, test_uuid,
};

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
        Err(Outcome::Refusal(refusal)) => refusal.reason().to_owned(),
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_for(argv: &[&str]) -> CommandResult<()> {
    let matches = spice::cli::command()
        .try_get_matches_from(argv)
        .expect("declared up operands parse");
    let leaf = spice::cli::selected_matches(&matches);
    let listen = requested_listen(leaf)?;
    requested_access(leaf, listen).map(|_| ())
}

fn access_refusal(argv: &[&str]) -> Refusal {
    match access_for(argv) {
        Ok(()) => panic!("{argv:?} must not admit access"),
        Err(Outcome::Refusal(refusal)) => refusal,
        Err(other) => panic!("{argv:?} must refuse rather than fault: {other:?}"),
    }
}

fn access_refusal_for(argv: &[&str]) -> String {
    access_refusal(argv).reason().to_owned()
}

/// The surface listens exactly where the operator requested or at one stable default.
///
/// Both halves are load-bearing. An operator reaching this surface from another
/// machine has to be able to say which address it answers on, and an operator
/// who says nothing has to get the same URL back after a restart: an ephemeral
/// port would hand them a new address every time, which no bookmark, open tab,
/// or written-down instruction survives.
#[test]
fn the_listen_address_is_what_was_asked_for_or_one_stable_default() {
    assert_eq!(listen_for(&["spice", "up"]), Ok(DEFAULT_LISTEN));
    assert_eq!(DEFAULT_LISTEN.port(), 8767);
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
    assert_eq!(invalid.reason(), "resident::auth-token-invalid");
    assert_eq!(
        invalid
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        None,
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

    assert_eq!(paths.runtime, common.join(".spice-rs"));
    assert_eq!(
        paths.journal,
        common.join(".spice-rs/data/spicejournal.sqlite3")
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
            assert_eq!(refusal.reason(), "resident::already-up");
            observed = refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .map(str::to_owned);
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

/// A liveness observation cannot impersonate the resident it is checking.
///
/// The probe hook freezes the shared observation lock after it lands. Only
/// then does startup make its exclusive attempt; its contention hook releases
/// the probe, proving the two operations overlapped at the kernel lock rather
/// than merely running in nearby threads. Startup must retry that observation,
/// while the same availability path must still distinguish its resulting live
/// exclusive holder from the available lock after release.
#[cfg(unix)]
#[test]
fn resident_start_survives_an_overlapping_liveness_probe() {
    let root = fixture("startup-probe-overlap");
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.runtime).expect("runtime directory");
    fs::write(&paths.resident, b"stale holder record\n").expect("existing resident lock file");
    let lock = LockFile::new(
        LockName::new("resident").expect("resident lock name"),
        &paths.resident,
    );
    let (probe_held, probe_landed) = mpsc::sync_channel(0);
    let (release_probe, released) = mpsc::sync_channel(0);

    std::thread::scope(|scope| {
        let observed_lock = lock.clone();
        let observer = scope.spawn(move || {
            observed_lock
                .availability_with(|| {
                    probe_held.send(()).expect("announce held probe");
                    released.recv().expect("startup releases probe");
                })
                .expect("probe answers")
        });
        probe_landed.recv().expect("probe lands before startup");

        let mut release_probe = Some(release_probe);
        let resident = ResidentLock::acquire_with(&paths, || {
            if let Some(release) = release_probe.take() {
                release.send(()).expect("release overlapping probe");
            }
        })
        .expect("probe-width contention cannot become already-up");

        assert_eq!(
            observer.join().expect("probe thread"),
            sill::Availability::Available
        );
        assert_eq!(
            lock.availability().expect("live resident probe"),
            sill::Availability::Held
        );
        drop(resident);
        assert_eq!(
            lock.availability().expect("released resident probe"),
            sill::Availability::Available
        );
    });

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

    assert_eq!(paths.runtime, common.join(".spice-rs"));
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
    assert_shutdown_ladder_and_endings(&snapshot);
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

/// Every fact one two-group shutdown is required to leave behind.
///
/// Held apart from the drive so the ladder counts and the endings can be read
/// as one statement of what the shutdown recorded, rather than as a tail on the
/// fixture that produced it.
fn assert_shutdown_ladder_and_endings(snapshot: &journal::FactSnapshot) {
    assert_eq!(
        fact_u64(snapshot, wire::FactKind::UpDraining, "occupants"),
        2
    );
    assert_eq!(
        fact_u64(snapshot, wire::FactKind::UpTerminating, "occupants"),
        1
    );
    assert_eq!(
        fact_u64(snapshot, wire::FactKind::UpKilling, "occupants"),
        1
    );
    let holds = |kind| snapshot.facts().iter().any(|fact| fact.fact.kind() == kind);
    assert!(holds(wire::FactKind::LaneDrained));
    // The forced group is the one that outlived TERM and reached the kill
    // stage, so it is recorded as killed. Requiring the absence of an exit
    // keeps the two groups distinguishable in the ledger: publishing both
    // would leave a reader unable to say which group ended which way.
    assert!(holds(wire::FactKind::LaneKilled));
    assert!(
        !holds(wire::FactKind::LaneExited),
        "a group this resident killed is not also recorded as an exit it chose"
    );
    let degraded = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::LaneDegraded)
        .count();
    assert_eq!(degraded, 2, "disk residue was not named once per lane");
}

/// A group that never trapped TERM ends at the terminate stage, and the ledger
/// says this resident ended it rather than crediting it with an exit.
///
/// Held apart from the two-group drive because the ending under test is the one
/// that drive cannot reach: its forced group traps TERM, so it always survives to
/// the kill stage. Terminate is the only mixed stage — an occupant that trapped
/// the signal chose a status, one that did not has none — and so the only stage
/// where reading the ending off the status can name the wrong ending.
///
/// `UpKilling` carrying no occupants is what makes this a statement about the
/// terminate stage. Without it the kill stage could have produced the same
/// `LaneKilled`, and the test would claim nothing the sibling does not.
#[test]
fn a_group_that_never_trapped_termination_is_not_credited_with_an_exit() {
    let (root, paths) = resident_fixture("terminated");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let mut seats = Seats::new();

    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    seats
        .seat(
            lane,
            occupant,
            process_spec("while :; do sleep 1; done", Duration::from_millis(100)),
        )
        .expect("untrapped group starts");
    seed_lane(&bus, epoch, lane, occupant);

    assert!(matches!(
        drive_shutdown(&bus, &paths, epoch, &mut seats),
        Outcome::Reply(_)
    ));
    let snapshot = bus.fact_snapshot().expect("facts read through bus");
    assert_eq!(
        fact_u64(&snapshot, wire::FactKind::UpTerminating, "occupants"),
        1,
        "the group outlived the drain bound and reached the terminate stage"
    );
    assert_eq!(
        fact_u64(&snapshot, wire::FactKind::UpKilling, "occupants"),
        0,
        "the terminate stage, not the kill stage, is what ended this group"
    );
    let holds = |kind| snapshot.facts().iter().any(|fact| fact.fact.kind() == kind);
    assert!(holds(wire::FactKind::LaneKilled));
    assert!(
        !holds(wire::FactKind::LaneExited),
        "a group that never trapped the signal chose no status to publish"
    );
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

struct ExitReporter(mpsc::SyncSender<()>);

impl RecordObserver for ExitReporter {
    fn observe(&self, _record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        Ok(())
    }

    fn exited(&self) {
        let _sent = self.0.send(());
    }
}

#[test]
fn kernel_exit_event_vacates_and_records_only_the_exact_occupant() {
    let (root, paths) = resident_fixture("occupant-exit");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    let (exited, exit_event) = mpsc::sync_channel(1);
    let observation = Observation::new(
        OutputStream::Stdout,
        1024,
        4,
        Arc::new(ExitReporter(exited)),
    )
    .expect("exit observation is valid");
    let mut spec = process_spec("exit 17", Duration::from_secs(5));
    spec.output = Output::new(
        StdinPolicy::Null,
        StreamPolicy::Capture { limit: 1024 },
        StreamPolicy::Null,
    );
    let mut seats = Seats::new();
    seats
        .seat_observed(lane, occupant, spec, observation)
        .expect("exiting occupant starts");
    seed_lane(&bus, epoch, lane, occupant);
    exit_event
        .recv_timeout(Duration::from_secs(5))
        .expect("waitid callback reports the exit");

    let exit = lane::capture::CaptureExit { lane, occupant };
    super::lane_surface::occupant_exited(&bus, epoch, &mut seats, exit)
        .expect("exit event commits");
    super::lane_surface::occupant_exited(&bus, epoch, &mut seats, exit)
        .expect("stale duplicate exit is inert");

    assert!(!seats.occupied(lane));
    let head = current_residency(&bus, epoch)
        .expect("exit residency reads")
        .into_body();
    let row = head
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .expect("exited lane remains durable");
    assert_eq!((row.occupant, row.disposition.as_str()), (None, "vacant"));
    let snapshot = bus.fact_snapshot().expect("exit facts read");
    let exits = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::LaneExited)
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 1, "the exact exit commits once");
    assert_eq!(exits[0].fact.body()["code"], 17);
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("exit fixture cleanup");
}

#[test]
fn failed_pending_inbox_start_parks_key_and_reports_exact_requeue() {
    let (root, paths) = resident_fixture("pending-dead-letter");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let lane = test_uuid(2);
    let occupant = test_uuid(3);
    let directive = test_uuid(4);
    seed_lane(&bus, epoch, lane, occupant);
    let observed = current_residency(&bus, epoch).expect("occupied residency reads");
    let mut vacant = observed.body().clone();
    let row = vacant
        .lanes
        .iter_mut()
        .find(|row| row.lane == lane)
        .expect("fixture lane exists");
    row.occupant = None;
    row.disposition = "vacant".to_owned();
    let mut ending =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("ending write starts");
    super::shutdown::guard_residency(&mut ending, &observed);
    let exited = ending
        .fact(
            wire::body::LaneExited {
                lane,
                occupant,
                code: 17,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("exit fact is valid");
    ending.residency_head(exited, move |_| vacant);
    commit(&bus, ending).expect("vacancy commits");
    let residency = current_residency(&bus, epoch).expect("vacant residency reads");
    let expires: wire::Instant = "2026-08-10T07:07:19.000Z".parse().expect("fixture expiry");
    let mut sent =
        write(Actor::Operator, epoch, Producer::CommandDoor).expect("steering write starts");
    sent.fact(
        wire::body::SteerSent {
            directive,
            key: "1kJK6N3R".to_owned(),
            lane,
            text: "do not lose me".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires,
        },
        FactCause::Existing(residency.position()),
    )
    .expect("steering fact is valid");
    commit(&bus, sent).expect("pending steering commits");
    let cursor = bus
        .task_snapshot()
        .expect("task snapshot reads")
        .cursor()
        .expect("pending fact establishes cursor");
    let failure = Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new("lifecycle::wake-launch-failed".to_owned(), None, None, None)
            .with_detail("repair the vendor launch"),
    ));

    let parked = super::steering::dead_letter_pending(&bus, &paths, epoch, lane, cursor, &failure)
        .expect("terminal start failure parks pending steering");

    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].key, "1kJK6N3R");
    assert_eq!(
        parked[0].requeue,
        format!("spice steer recover {directive}")
    );
    let snapshot = bus.fact_snapshot().expect("dead-letter facts read");
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("dead letter replays");
    assert_eq!(
        ledger
            .directive(directive)
            .expect("directive remains durable")
            .disposition(),
        wire::Disposition::DeadLettered,
    );
    let parked_fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == wire::FactKind::SteerDeadLettered)
        .expect("distinct dead-letter fact exists");
    assert_eq!(parked_fact.fact.body()["key"], "1kJK6N3R");
    assert_eq!(
        parked_fact.fact.body()["requeue"],
        format!("spice steer recover {directive}")
    );
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("dead-letter fixture cleanup");
}

#[test]
fn startup_migrates_one_complete_legacy_subscription_table_and_refuses_partial_authority() {
    let lineage = test_uuid(2);
    let sibling = test_uuid(5);
    let row = |lane, lineage_id, life: &str| wire::LaneResidency {
        lane,
        lineage_id,
        name: None,
        worktree: format!("/fixture/{lane}"),
        life: life.to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: None,
        lineage: Vec::new(),
        capabilities: Vec::new(),
        session: None,
        disposition: "vacant".to_owned(),
    };
    let lanes = vec![
        row(test_uuid(3), lineage, "drive"),
        row(test_uuid(4), lineage, "drive"),
        row(test_uuid(6), sibling, "steer"),
    ];

    let migrated =
        super::startup_subscriptions(&lanes, Vec::new()).expect("one absent legacy table migrates");
    assert_eq!(
        migrated,
        vec![
            wire::LineageSubscription {
                lineage_id: lineage,
                policy: wire::intent::LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            },
            wire::LineageSubscription {
                lineage_id: sibling,
                policy: wire::intent::LineageLife::Steer,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            },
        ]
    );
    let mut projected = migrated.clone();
    projected[0].auto_projects = vec!["task.derived".to_owned()];
    assert_eq!(
        super::startup_subscriptions(&lanes, projected)
            .expect("the current complete table is preserved"),
        migrated,
        "startup discards derived auto projects before fact replay",
    );

    let Err(Outcome::Fault(fault)) =
        super::startup_subscriptions(&lanes, vec![migrated[0].clone()])
    else {
        panic!("a partial current subscription table must fault")
    };
    assert_eq!(fault.reason, "lineage::subscription-misaligned");

    let mut mixed = lanes.clone();
    mixed[1].life = "steer".to_owned();
    let Err(Outcome::Fault(fault)) = super::startup_subscriptions(&mixed, Vec::new()) else {
        panic!("mixed legacy lives must not choose an implicit policy")
    };
    assert_eq!(fault.reason, "lineage::subscription-ambiguous");

    let duplicated = vec![
        migrated[0].clone(),
        migrated[0].clone(),
        migrated[1].clone(),
    ];
    let Err(Outcome::Fault(fault)) = super::startup_subscriptions(&lanes, duplicated) else {
        panic!("duplicate current subscription authority must fault")
    };
    assert_eq!(fault.reason, "lineage::subscription-ambiguous");
}

fn assert_lineage_configuration_fact(bus: &Bus, lineage: CanonicalUuid) {
    let snapshot = bus.canvas_snapshot().expect("configured journal reads");
    let configured = snapshot
        .facts()
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == wire::FactKind::LineageConfigured)
        .expect("lineage configuration is durable");
    assert_eq!(
        configured.fact.body().get("lineage_id"),
        Some(&serde_json::Value::String(lineage.to_string())),
    );
    assert_eq!(
        configured.fact.body().get("from"),
        Some(&serde_json::json!("drive"))
    );
    assert_eq!(
        configured.fact.body().get("life"),
        Some(&serde_json::json!("drain"))
    );
    assert_eq!(configured.fact.body().get("from_manual_projects"), None);
    assert_eq!(
        configured.fact.body().get("manual_projects"),
        Some(&serde_json::json!(["task.manual"]))
    );
}

#[test]
fn lineage_configuration_changes_only_the_typed_task_subscription() {
    let (root, paths) = resident_fixture("lineage-configuration");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let lineage = test_uuid(2);
    let first_lane = test_uuid(3);
    let first_occupant = test_uuid(4);
    let second_lane = test_uuid(5);
    let second_occupant = test_uuid(6);
    let mut seats = Seats::new();
    seats
        .seat(
            first_lane,
            first_occupant,
            process_spec("sleep 0.05", Duration::from_secs(5)),
        )
        .expect("first supervised occupant starts");
    seats
        .seat(
            second_lane,
            second_occupant,
            process_spec("sleep 0.05", Duration::from_secs(5)),
        )
        .expect("second supervised occupant starts");
    seed_lane_in_lineage(&bus, epoch, first_lane, lineage, first_occupant);
    seed_lane_in_lineage(&bus, epoch, second_lane, lineage, second_occupant);

    let request = wire::intent::LineageConfigure {
        topology_revision: current_residency(&bus, epoch)
            .expect("fixture residency reads")
            .body()
            .topology_revision,
        lineage_id: lineage,
        member: None,
        life: wire::intent::LineageLife::Drain,
        manual_projects: Some(vec!["task.manual".to_owned()]),
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id: test_uuid(7),
        epoch,
        kind: CommandKind::LineageConfigure,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("lineage configuration encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator lineage configuration admits");

    let projects =
        board::ProjectPolicy::new(Vec::new(), Vec::new(), 2, 4).expect("fixture project policy");
    let Outcome::Reply(reply) =
        super::lane_surface::configure(&bus, epoch, &seats, &projects, &command, &request)
    else {
        panic!("lineage configuration replies");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    let CommandAnswer::Lane(answer) = reply.body else {
        panic!("lineage configuration returns the authoritative lane table");
    };
    assert_eq!(answer.lanes.len(), 2);
    assert!(answer.lanes.iter().all(|row| row.lineage_id == lineage));
    assert!(answer.lanes.iter().all(|row| row.life == "drive"));
    assert_eq!(
        answer.subscriptions,
        vec![wire::LineageSubscription {
            lineage_id: lineage,
            policy: wire::intent::LineageLife::Drain,
            manual_projects: vec!["task.manual".to_owned()],
            auto_projects: Vec::new(),
        }]
    );
    assert_eq!(answer.lanes[0].occupant, Some(first_occupant));
    assert_eq!(answer.lanes[1].occupant, Some(second_occupant));
    assert!(
        answer.lanes.iter().all(|row| row.disposition == "occupied"),
        "task-subscription drain never performs LaneAction::Drain",
    );
    assert!(seats.occupied(first_lane));
    assert!(seats.occupied(second_lane));
    let durable = current_residency(&bus, epoch)
        .expect("configured residency reads")
        .into_body();
    assert!(durable.lanes.iter().all(|row| row.life == "drive"));
    assert_eq!(durable.subscriptions, answer.subscriptions);
    assert_lineage_configuration_fact(&bus, lineage);
    for lane in [first_lane, second_lane] {
        seats
            .vacate(lane)
            .expect("configured occupant remains supervised")
            .wait()
            .expect("fixture occupant exits");
    }
    bus.verify().expect("configured authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

/// Arrangement is the one command a drag commits, and it carries its own
/// fixture, so its cases are proven in a module of their own.
#[path = "arrangement_tests.rs"]
mod arrangement;

fn arrange_lane_for_test(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &Seats,
    id: CanonicalUuid,
    lane: CanonicalUuid,
    before: CanonicalUuid,
) -> wire::intent::LaneAnswer {
    let request = wire::intent::LaneArrange {
        topology_revision: current_residency(bus, epoch)
            .expect("fixture residency reads")
            .body()
            .topology_revision,
        lane,
        before,
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::LaneArrange,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("lane arrangement encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
    })
    .expect("operator lane arrangement admits");
    let Outcome::Reply(reply) = super::lane_surface::arrange(bus, epoch, seats, &command, &request)
    else {
        panic!("lane arrangement replies");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    let CommandAnswer::Lane(answer) = reply.body else {
        panic!("lane arrangement returns the authoritative lane table");
    };
    *answer
}

fn lane_order(answer: &wire::intent::LaneAnswer) -> Vec<CanonicalUuid> {
    answer.lanes.iter().map(|row| row.lane).collect()
}

fn assert_transferred_arrangement(
    transferred: &wire::intent::LaneAnswer,
    destination: CanonicalUuid,
) {
    assert_eq!(
        lane_order(transferred),
        vec![test_uuid(7), test_uuid(9), test_uuid(3), test_uuid(5)],
        "the transferred member is inserted immediately before its destination target",
    );
    let transferred_row = &transferred.lanes[1];
    assert_eq!(transferred_row.lane, test_uuid(9));
    assert_eq!(transferred_row.occupant, Some(test_uuid(10)));
    assert_eq!(transferred_row.lineage_id, destination);
    assert_eq!(
        transferred
            .subscriptions
            .iter()
            .map(|subscription| subscription.lineage_id)
            .collect::<Vec<_>>(),
        vec![destination],
        "moving the last member removes the empty source lineage subscription",
    );
}

fn assert_durable_arrangement(
    bus: &Bus,
    epoch: CanonicalUuid,
    transferred: &wire::intent::LaneAnswer,
    source_lineage: CanonicalUuid,
    destination_lineage: CanonicalUuid,
) {
    let durable = current_residency(bus, epoch)
        .expect("arranged residency reads")
        .into_body();
    let identities = |lanes: &[wire::intent::LaneAnswerRow]| {
        lanes
            .iter()
            .map(|row| (row.lane, row.lineage_id, row.occupant))
            .collect::<Vec<_>>()
    };
    let durable_identities = durable
        .lanes
        .iter()
        .map(|row| (row.lane, row.lineage_id, row.occupant))
        .collect::<Vec<_>>();
    assert_eq!(durable_identities, identities(&transferred.lanes));
    let arranged = bus
        .fact_snapshot()
        .expect("lane arrangement facts read")
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::LaneArranged)
        .map(|fact| fact.fact.body().clone())
        .collect::<Vec<_>>();
    assert_eq!(arranged.len(), 2);
    assert_eq!(arranged[1]["lane"], serde_json::json!(test_uuid(9)));
    assert_eq!(
        arranged[1]["from_lineage_id"],
        serde_json::json!(source_lineage)
    );
    assert_eq!(
        arranged[1]["lineage_id"],
        serde_json::json!(destination_lineage)
    );
    assert_eq!(arranged[1]["before"], serde_json::json!(test_uuid(3)));
}

#[test]
fn lane_arrangement_durably_reorders_and_transfers_the_existing_member() {
    let (root, paths) = resident_fixture("lane-arrangement");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let epoch = test_uuid(1);
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
    let first_lineage = test_uuid(2);
    let second_lineage = test_uuid(12);
    let members = [
        (test_uuid(3), test_uuid(4), first_lineage),
        (test_uuid(5), test_uuid(6), first_lineage),
        (test_uuid(7), test_uuid(8), first_lineage),
        (test_uuid(9), test_uuid(10), second_lineage),
    ];
    let mut seats = Seats::new();
    for (lane, occupant, lineage) in members {
        seats
            .seat(
                lane,
                occupant,
                process_spec("sleep 0.05", Duration::from_secs(5)),
            )
            .expect("supervised member starts");
        seed_lane_in_lineage(&bus, epoch, lane, lineage, occupant);
    }

    let same_team = arrange_lane_for_test(
        &bus,
        epoch,
        &seats,
        test_uuid(20),
        test_uuid(7),
        test_uuid(3),
    );
    assert_eq!(
        lane_order(&same_team),
        vec![test_uuid(7), test_uuid(3), test_uuid(5), test_uuid(9)],
        "the moved member is inserted immediately before its same-team target",
    );
    assert_eq!(same_team.lanes[0].lineage_id, first_lineage);

    let transferred = arrange_lane_for_test(
        &bus,
        epoch,
        &seats,
        test_uuid(21),
        test_uuid(9),
        test_uuid(3),
    );
    assert_transferred_arrangement(&transferred, first_lineage);
    assert_durable_arrangement(&bus, epoch, &transferred, second_lineage, first_lineage);

    for (lane, _, _) in members {
        seats
            .vacate(lane)
            .expect("arranged member remains supervised")
            .wait()
            .expect("fixture occupant exits");
    }
    bus.verify().expect("arranged authority verifies");
    bus.shutdown().expect("arrangement fixture bus stops");
    fs::remove_dir_all(root).expect("arrangement fixture cleanup");
}
