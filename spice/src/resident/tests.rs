//! Runtime and shutdown regressions for the split resident surface.
//! Long startup and media cases live in child modules to keep this root bounded.

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
#[path = "tests/message_backlog.rs"]
mod message_backlog;
#[path = "tests/message_unavailable.rs"]
mod message_unavailable;
#[path = "tests/startup.rs"]
mod startup;
#[path = "tests/startup_window.rs"]
mod startup_window;

use canvas_announcement::{
    fact_u64, process_spec, resident_fixture, seed_lane, seed_lane_in_lineage,
    seed_lane_in_worktree, test_uuid,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn fixture(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-resident-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn consent_and_effort_requests_read_one_current_generation_at_constant_cost() {
    let (root, paths) = resident_fixture("current-read-materializations");
    let mut authority = journal::Journal::open(&paths.journal).expect("fixture authority opens");
    authority
        .apply(effort_recognition("authority", 1))
        .expect("authority fact commits");
    let bus = Bus::start(authority).expect("authority bus starts");
    let policy = consent::Policy::new(consent::Answer::Queue, Vec::new())
        .expect("empty consent policy is valid");
    let reads = read_materializations::CurrentReads::start(&bus, &paths, &policy)
        .expect("read materializations start");

    let mut unrelated = JournalWrite::new(
        Actor::Spice,
        "recovery:unrelated-history"
            .parse()
            .expect("fixture recovery origin"),
        "2030-04-12T18:45:00.271Z".parse().expect("fixture instant"),
        Producer::Judge,
    );
    for index in 0..512 {
        unrelated
            .fact(
                wire::body::TasteCleared {
                    taste: format!("unrelated-{index}"),
                    locus: "request-cost-fixture".to_owned(),
                },
                FactCause::Root,
            )
            .expect("unrelated fact builds");
    }
    assert!(matches!(
        bus.commit(unrelated).expect("unrelated history commits"),
        CommitOutcome::Committed(_)
    ));
    reads
        .refresh()
        .expect("current generation refresh schedules");
    let deadline = Monotonic::now() + Duration::from_secs(2);
    loop {
        if reads
            .current()
            .is_ok_and(|generation| generation.cursor() == seq(513))
        {
            break;
        }
        assert!(
            Monotonic::now() < deadline,
            "read materialization did not reach unrelated-history cursor"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let unavailable_fold = paths.folds.with_extension("unavailable");
    fs::rename(&paths.folds, &unavailable_fold).expect("fold store moves out of request reach");
    let barriers = reads.request_barriers();
    let effort = effort_surface::read(&reads, &wire::intent::EffortRead { task: None });
    let consent = policy_surface::read(&reads, &policy);
    assert_eq!(reads.request_barriers() - barriers, 2);
    for outcome in [effort, consent] {
        let Outcome::Reply(Reply { position, .. }) = outcome else {
            panic!("current materialized request did not reply")
        };
        assert_eq!(position, ReplyPosition::Snapshot(seq(513)));
    }
    assert!(
        !paths.folds.exists(),
        "request path reopened the unavailable fold store"
    );

    drop(reads);
    drop(bus);
    fs::remove_dir_all(root).expect("remove effort authority fixture");
}

fn effort_recognition(label: &str, second: u8) -> JournalWrite {
    let at = format!("2030-04-12T18:44:{second:02}.271Z")
        .parse()
        .expect("fixture instant");
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("recovery:{label}")
            .parse()
            .expect("fixture recovery origin"),
        at,
        Producer::ResidentRecognition,
    );
    write
        .fact(
            wire::body::JournalBacked {
                bundle: label.to_owned(),
                manifest: Vec::new(),
            },
            FactCause::Root,
        )
        .expect("fixture recognition fact");
    write
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
    seed_lane(&bus, epoch, drained_lane, drained_occupant, &root);

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
    seed_lane(&bus, epoch, killed_lane, killed_occupant, &root);

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
    seed_lane(&bus, epoch, lane, occupant, &root);

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
    seed_lane(&bus, epoch, lane, occupant, &root);

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
    seed_lane(&bus, epoch, lane, occupant, &root);
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
    seed_lane(&bus, epoch, lane, occupant, &root);
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
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("steering instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("steering origin");
    let mut sent = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let body = wire::body::SteerSent {
        directive,
        fanout: None,
        key: "1kJK6N3R".to_owned(),
        lane,
        text: "do not lose me".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires,
    };
    let pending = sent
        .fact(body.clone(), FactCause::Existing(residency.position()))
        .expect("steering fact is valid");
    sent.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("steering head is valid")
    });
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
    let facts = bus.fact_snapshot().expect("configured journal reads");
    let configured = facts
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
    seed_lane_in_lineage(&bus, epoch, first_lane, lineage, first_occupant, &root);
    seed_lane_in_lineage(&bus, epoch, second_lane, lineage, second_occupant, &root);

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
        seed_lane_in_lineage(&bus, epoch, lane, lineage, occupant, &root);
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
