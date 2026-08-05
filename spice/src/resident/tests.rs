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
