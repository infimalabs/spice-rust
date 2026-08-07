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
    access_refusal(argv).reason
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
    let request = wire::intent::LaneArrange { lane, before };
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
    seed_lane_in_lineage(bus, epoch, lane, lane, occupant);
}

fn seed_lane_in_lineage(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    lineage_id: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    let current = current_residency(bus, epoch).expect("current residency");
    let mut head = current.body().clone();
    head.lanes.push(wire::LaneResidency {
        lane,
        lineage_id,
        name: None,
        worktree: "/fixture".to_owned(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![test_uuid(9)],
        session: None,
        disposition: "occupied".to_owned(),
    });
    if !head
        .subscriptions
        .iter()
        .any(|subscription| subscription.lineage_id == lineage_id)
    {
        head.subscriptions.push(wire::LineageSubscription {
            lineage_id,
            policy: wire::intent::LineageLife::Drive,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        });
        head.subscriptions
            .sort_by_key(|subscription| subscription.lineage_id);
    }
    head.lanes.sort_by_key(|lane| lane.lane);
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id,
                capability: test_uuid(9),
                name: None,
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
        None,
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
        None,
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

/// An authenticated opening keeps its bearer outside the HTTP origin.
#[test]
fn authenticated_opening_at_start_carries_the_token_as_a_fragment() {
    const TOKEN: &str = "correct-horse-battery-staple";

    let (root, path, discovery) = published_discovery("announce-authenticated", 48234);
    let mut dialled = None;
    let mut announced = Vec::new();
    let mut diagnostics = Vec::new();

    announce_canvas(
        &path,
        Some(TOKEN),
        true,
        || {},
        |endpoint| {
            dialled = Some(endpoint.to_owned());
            Ok(())
        },
        &mut announced,
        &mut diagnostics,
    );

    let opening = format!("{}#{TOKEN}", discovery.endpoint);
    assert_eq!(dialled.as_deref(), Some(opening.as_str()));
    assert_eq!(
        String::from_utf8(announced).expect("announcement is text"),
        format!("canvas endpoint: {opening}\n")
    );
    assert_eq!(
        opening.split_once('#').expect("opening has fragment").0,
        discovery.endpoint,
        "the token must not become part of the HTTP origin"
    );
    assert!(diagnostics.is_empty());
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
        None,
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
fn durable_capture_projects_current_occupant_tool_and_prose_with_event_identity() {
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
            listen::Event::Tool(listen::Tool {
                call_id: "tool-1".to_owned(),
                moment: listen::ToolMoment::Call {
                    name: "Bash".to_owned(),
                    arguments: r#"{"command":"git status --short","description":"read the tree"}"#
                        .to_owned(),
                },
                locus,
            }),
            listen::Event::Prose(listen::Prose {
                text: "agent-authored answer".to_owned(),
                phase: None,
                locus,
            }),
        ]),
    };
    let mut seats = Seats::new();
    seats
        .seat(
            record.lane,
            record.occupant,
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("capture fixture occupant starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();

    accept_capture_record(
        &bus,
        test_uuid(4),
        &mut seats,
        &publisher,
        &record,
        &mut previews,
        &mut diagnostics,
    )
    .expect("current capture projects");
    assert_tool_and_prose_activity(&mut updates, &record);
    assert_stale_capture_ignored(
        &bus,
        &mut seats,
        &publisher,
        &mut updates,
        &record,
        &mut previews,
        &mut diagnostics,
    );
    seats
        .vacate(record.lane)
        .expect("replacement occupant remains supervised")
        .wait()
        .expect("replacement occupant stops");
    drop(source);
    bus.shutdown().expect("message bus stops");
}

fn assert_tool_and_prose_activity(
    updates: &mut tokio::sync::broadcast::Receiver<wire::AgentMessage>,
    record: &lane::capture::CaptureRecord,
) {
    let tool = updates.try_recv().expect("tool activity publishes once");
    assert_eq!(tool.lane, record.lane);
    assert_eq!(tool.author, record.occupant);
    assert_eq!(tool.activity, wire::AgentActivityKind::Tool);
    assert_eq!(tool.text, "Bash: git status --short");
    assert_eq!(
        tool.locus,
        wire::MessageLocus {
            member: 4,
            offset: 120,
            length: 48,
            event: 1,
        }
    );

    let prose = updates.try_recv().expect("prose publishes once");
    assert_eq!(prose.lane, record.lane);
    assert_eq!(prose.author, record.occupant);
    assert_eq!(prose.activity, wire::AgentActivityKind::Prose);
    assert_eq!(prose.text, "agent-authored answer");
    assert_eq!(
        prose.locus,
        wire::MessageLocus {
            member: 4,
            offset: 120,
            length: 48,
            event: 2,
        }
    );
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

fn assert_stale_capture_ignored(
    bus: &bus::Bus,
    seats: &mut Seats,
    publisher: &impl AgentMessagePublisher,
    updates: &mut tokio::sync::broadcast::Receiver<wire::AgentMessage>,
    record: &lane::capture::CaptureRecord,
    previews: &mut CallPreviews,
    diagnostics: &mut Vec<u8>,
) {
    seats
        .vacate(record.lane)
        .expect("original occupant remains supervised")
        .wait()
        .expect("original occupant stops");
    seats
        .seat(
            record.lane,
            test_uuid(7),
            process_spec(
                "exec /usr/bin/tail -f /dev/null",
                Duration::from_millis(100),
            ),
        )
        .expect("replacement occupant starts");
    let locus = listen::Locus::new(4, 180, 48);
    let stale = lane::capture::CaptureRecord {
        lane: record.lane,
        occupant: record.occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events(vec![listen::Event::Prose(listen::Prose {
            text: "stale prior-occupant answer".to_owned(),
            phase: None,
            locus,
        })]),
    };
    accept_capture_record(
        bus,
        test_uuid(4),
        seats,
        publisher,
        &stale,
        previews,
        diagnostics,
    )
    .expect("stale capture is ignored");
    assert!(matches!(
        updates.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(diagnostics.is_empty());
}

struct UnavailableMessagePublisher;

impl AgentMessagePublisher for UnavailableMessagePublisher {
    fn publish(
        &self,
        _message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Err(canvas::stream::MessagePublishError::Unavailable)
    }
}

#[test]
fn unavailable_canvas_messages_are_diagnostic_and_capture_remains_live() {
    let bus = Bus::start(journal::Journal::in_memory().expect("capture authority opens"))
        .expect("capture bus starts");
    let epoch = test_uuid(4);
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec("sleep 0.05", Duration::from_secs(5)),
        )
        .expect("capture fixture group starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();

    for (offset, text) in [(120, "first answer"), (180, "later answer")] {
        let locus = listen::Locus::new(4, offset, 48);
        let record = lane::capture::CaptureRecord {
            lane,
            occupant,
            stream: vendor::RecordStream::Stdout,
            boundary: sill::RecordBoundary::Complete,
            locus,
            declaration: None,
            decoded: listen::Decoded::Events(vec![listen::Event::Prose(listen::Prose {
                text: text.to_owned(),
                phase: None,
                locus,
            })]),
        };

        accept_capture_record(
            &bus,
            epoch,
            &mut seats,
            &UnavailableMessagePublisher,
            &record,
            &mut previews,
            &mut diagnostics,
        )
        .expect("a failed Canvas view leaves the capture handler live");
    }

    assert_eq!(
        String::from_utf8(diagnostics).expect("diagnostic is text"),
        concat!(
            "canvas message unavailable: canvas::message-publish-failed: ",
            "canvas message retention is unavailable\n",
            "canvas message unavailable: canvas::message-publish-failed: ",
            "canvas message retention is unavailable\n",
        )
    );
    seats
        .vacate(lane)
        .expect("capture fixture remains supervised")
        .wait()
        .expect("capture fixture stops");
    bus.shutdown().expect("capture bus stops");
}

/// A generation that was never published is a diagnostic, not a silent start.
///
/// This is the operator's first start, before anything has been published, and
/// it reaches the announcement through `DiscoveryRead::open` rather than the
/// identity recheck the replaced-generation case above exercises.
///
/// Both arrive as *faults*: the announcement boundary accepts only a typed
/// endpoint or fault, so no unreachable refusal arm can intercept this route.
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
        None,
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
