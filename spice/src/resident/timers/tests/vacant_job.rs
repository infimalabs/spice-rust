//! Renewal of a claim whose occupant ended its turn on a kept job.

use std::ffi::OsString;
use std::time::Duration;

use lane::occupant::Seats;
use sill::Environment;

use crate::resident::lane_surface::{
    AdvisoryRtkFixture, AutomaticWake, BeginAutomaticWake, begin_automatic_wake,
    finish_automatic_wake,
};

use super::super::super::supervisor::terminal::{Job, Jobs};
use super::*;

/// How long a test waits for a keeper to publish its state.
const KEEPER_BOUND: Duration = Duration::from_secs(2);
const PREDECESSOR_SESSION: &str = "018f7777-7777-7444-8888-999999999999";

struct Supervisors(PathBuf);

impl Supervisors {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-timer-{name}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_millisecond()
        ));
        std::fs::create_dir_all(&root).expect("scratch supervisors");
        Self(root)
    }

    /// Start one job of the lane; its keeper wakes nobody when it ends.
    fn start(&self, lane: CanonicalUuid, command: &str, timeout: Option<Duration>) -> Job {
        let jobs = Jobs::new(self.0.join(lane.to_string()).join("jobs"));
        let job = jobs
            .start(
                command,
                ["/bin/sh", "-c", command].map(OsString::from).to_vec(),
                &self.0,
                &Environment::empty(),
                timeout,
                None,
                &[],
            )
            .expect("the job starts");
        let started = std::time::Instant::now();
        while job.state().expect("job state reads").is_none() {
            assert!(started.elapsed() < KEEPER_BOUND, "the keeper publishes");
            std::thread::sleep(Duration::from_millis(5));
        }
        job
    }

    fn timers(&self, fixture: &Fixture) -> Runtime {
        Runtime::start(
            fixture.projects.clone(),
            steer::Config::default(),
            self.0.clone(),
        )
        .expect("timer runtime starts")
    }
}

impl Drop for Supervisors {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// End the occupant's seat while its claim stays active.
fn vacate(fixture: &Fixture) {
    let observed = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    let mut vacant = observed.body().clone();
    vacant.lanes[0].occupant = None;
    vacant.lanes[0].disposition = "vacant".to_owned();
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{}", fixture.authority.epoch)
            .parse()
            .expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical"),
        Producer::ResidentRecognition,
    );
    let exited = write
        .fact(
            body::LaneExited {
                lane: fixture.authority.lane,
                occupant: fixture.authority.occupant,
                code: 0,
            },
            FactCause::Root,
        )
        .expect("exit fact builds");
    write.residency_head(exited, move |_| vacant);
    assert!(matches!(
        fixture.bus.commit(write).expect("vacancy commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn renew_once(timers: &mut Runtime, fixture: &Fixture) -> Fired {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime.block_on(timers.recv()).expect("due renewal fires");
    assert_eq!(event.kind(), TimerKind::ClaimRenew);
    timers
        .consume(&fixture.bus, event)
        .expect("renewal consumes")
}

fn lease(fixture: &Fixture) -> wire::LeaseHead {
    let stamp = fixture.authority.born();
    fixture
        .bus
        .read(move |read| Ok(read.lease_head(&stamp)?.expect("lease head exists")))
        .expect("lease reads")
        .body()
        .clone()
}

#[test]
fn a_vacant_lane_keeps_its_claim_rolling_while_a_regular_job_runs() {
    let fixture = automatic_fixture();
    let supervisors = Supervisors::new("regular");
    let job = supervisors.start(
        fixture.authority.lane,
        "sleep 30",
        Some(Duration::from_secs(600)),
    );
    vacate(&fixture);
    let mut timers = supervisors.timers(&fixture);
    timers
        .synchronize(&fixture.bus)
        .expect("vacant claim with a running job arms");
    assert_eq!(timers.renewals.len(), 1);

    assert_eq!(renew_once(&mut timers, &fixture), Fired::ClaimRenew);
    let renewed = lease(&fixture);
    assert_eq!(
        renewed.expiry,
        shifted(
            renewed.renewal,
            crate::board_command::AUTOMATIC_ROLLING_SECONDS
        ),
        "a job far from its deadline leaves the full rolling horizon",
    );
    timers
        .synchronize(&fixture.bus)
        .expect("renewed vacant claim re-arms");
    assert_eq!(timers.renewals.len(), 1);

    move_residency(&fixture, fixture.authority.epoch, uuid(24));
    timers
        .synchronize(&fixture.bus)
        .expect("another occupant disarms the renewal");
    assert!(
        timers.renewals.is_empty(),
        "the job never rolls a claim while another occupant holds the seat",
    );

    job.stop().expect("the job stops");
    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn a_vacant_lane_with_only_a_service_and_ended_jobs_stops_rolling() {
    let fixture = automatic_fixture();
    let supervisors = Supervisors::new("service");
    let service = supervisors.start(fixture.authority.lane, "sleep 30", None);
    let ended = supervisors.start(
        fixture.authority.lane,
        "exit 0",
        Some(Duration::from_secs(600)),
    );
    let started = std::time::Instant::now();
    while ended.ended().is_none() {
        assert!(started.elapsed() < KEEPER_BOUND, "the finite job ends");
        std::thread::sleep(Duration::from_millis(5));
    }
    vacate(&fixture);
    let mut timers = supervisors.timers(&fixture);
    timers
        .synchronize(&fixture.bus)
        .expect("vacant claim folds");

    assert!(timers.renewals.is_empty());
    assert_eq!(
        timers
            .claims
            .get(&fixture.authority.task)
            .expect("lapse timer remains armed")
            .authority
            .expiry,
        fixture.authority.expiry,
        "the claim lapses on the lease it already carries",
    );

    service.stop().expect("the service stops");
    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn a_successor_wake_carries_the_claim_after_a_vacant_job_times_out() {
    let advisory = AdvisoryRtkFixture::new();
    let fixture = automatic_fixture_for(&advisory);
    let supervisors = Supervisors(advisory.paths.supervisors.clone());
    let timeout = Duration::from_millis(200);
    let job = supervisors.start(fixture.authority.lane, "sleep 1", Some(timeout));
    let mut timers = supervisors.timers(&fixture);

    let heartbeat = Instant::new(jiff::Timestamp::now()).expect("fixture heartbeat is canonical");
    assert_eq!(
        renew_at(&mut timers, &fixture, heartbeat),
        Fired::ClaimRenew
    );
    let renewed = lease(&fixture);
    assert_eq!(
        renewed.expiry,
        shifted(heartbeat, crate::board_command::AUTOMATIC_ROLLING_SECONDS)
    );

    let waited = std::time::Instant::now();
    while job.ended().is_none() {
        assert!(
            waited.elapsed() < KEEPER_BOUND,
            "the timed-out keeper publishes its ending"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let capabilities = serve::axum::Capabilities::default();
    let discovery = serve::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: advisory.capability,
        epoch: advisory.epoch,
    };
    crate::resident::steering::publish_lane_surface(
        &advisory.paths,
        &discovery,
        advisory.lane,
        advisory.capability,
    )
    .expect("fixture lane surface publishes");
    let runtime = advisory.lane_runtime(&fixture.bus, &capabilities, &discovery);
    let snapshot = fixture
        .bus
        .task_snapshot()
        .expect("wake decision snapshot reads");
    let successor = uuid(42);
    let launch = match begin_automatic_wake(
        &fixture.bus,
        &advisory.paths,
        advisory.epoch,
        &Seats::new(),
        &runtime,
        &steer::Config::default(),
        advisory.lane,
        successor,
        Some(&snapshot),
        None,
        false,
    )
    .expect("successor wake is admitted")
    {
        BeginAutomaticWake::Launch(launch) => launch,
        BeginAutomaticWake::Unstarted(outcome) => {
            panic!("successor launch did not spawn: {outcome:?}")
        }
    };
    let (launch, pending) = launch.into_parts();
    let wake_started = Instant::new(jiff::Timestamp::now()).expect("wake instant is canonical");
    let mut seats = Seats::new();
    let outcome = finish_automatic_wake(
        &fixture.bus,
        &advisory.paths,
        advisory.epoch,
        &mut seats,
        pending,
        launch.establish(),
    )
    .expect("successor wake commits");
    assert!(matches!(outcome, AutomaticWake::Started));

    let carried = fixture.bus.task_snapshot().expect("carried claim reads");
    let carried = carried
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == fixture.authority.born())
        .expect("carried task remains");
    let claim = carried.claim().expect("carried claim remains");
    let promoted = carried.lease().expect("carried lease remains");
    assert_eq!(claim.body().occupant, successor);
    assert_eq!(claim.position(), promoted.position());
    assert!(
        promoted.body().expiry > renewed.expiry,
        "the successor receives a new rolling window after the job timeout"
    );
    assert!(
        promoted.body().expiry
            >= shifted(
                wake_started,
                crate::board_command::AUTOMATIC_ROLLING_SECONDS
            )
    );
    let facts = fixture.bus.fact_snapshot().expect("carried facts read");
    let carried_fact = facts
        .facts()
        .iter()
        .find(|fact| fact.seq == claim.position())
        .expect("claim head names its carried fact");
    assert_eq!(carried_fact.fact.kind(), wire::FactKind::ClaimCarried);

    seats.release_all();
    crate::resident::steering::remove_lane_surface(&advisory.paths, advisory.lane);
    job.stop().expect("the job stops");
    drop(timers);
    fixture.bus.verify().expect("fixture authority verifies");
    fixture.bus.shutdown().expect("fixture bus stops");
}

fn automatic_fixture_for(advisory: &AdvisoryRtkFixture) -> Fixture {
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(
        now,
        -(crate::board_command::AUTOMATIC_HEARTBEAT_SECONDS + 1),
    );
    let authority = Authority {
        task: task(),
        lane: advisory.lane,
        occupant: advisory.occupant,
        epoch: advisory.epoch,
        worktree: advisory.root.to_string_lossy().into_owned(),
        start,
        expiry: shifted(start, crate::board_command::AUTOMATIC_PROVISIONAL_SECONDS),
        maximum: shifted(start, crate::board_command::AUTOMATIC_MAXIMUM_SECONDS),
    };
    let mut vacant = lane_residency(&authority);
    vacant.occupant = None;
    vacant.disposition = "vacant".to_owned();
    let record = format!(
        r#"{{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{{"sessionId":"{PREDECESSOR_SESSION}"}}}}"#
    );
    let tape = tape::Tape::open(
        advisory
            .paths
            .tape
            .join(advisory.lane.to_string())
            .join(advisory.vendor.as_str()),
    )
    .expect("fixture tape opens");
    let locus = tape
        .tee(record.as_bytes())
        .expect("fixture session declaration commits");
    vacant.session = Some(wire::LaneSession {
        lane: advisory.lane,
        occupant: advisory.occupant,
        worktree: authority.worktree.clone(),
        vendor: advisory.vendor.clone(),
        launch: None,
        epoch: advisory.epoch,
        stream: "stdout".to_owned(),
        identity: PREDECESSOR_SESSION.to_owned(),
        locus: wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        },
    });
    let mut journal = journal::Journal::in_memory().expect("fixture journal opens");
    journal
        .apply(seed_write_with_lanes(&authority, vec![vacant]))
        .expect("fixture vacant automatic authority seeds");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let residency = bus
        .residency_head()
        .expect("fixture residency reads")
        .expect("fixture residency exists")
        .body()
        .clone();
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let mut recognition = JournalWrite::new(
        Actor::Spice,
        format!("resident:{}", advisory.epoch)
            .parse()
            .expect("fixture resident origin"),
        at,
        Producer::CommandDoor,
    );
    let seated = recognition
        .fact(
            body::UpSeated {
                epoch: advisory.epoch,
            },
            FactCause::Root,
        )
        .expect("fixture resident fact builds");
    recognition.residency_head(seated, move |_| residency);
    assert!(matches!(
        bus.commit(recognition)
            .expect("fixture resident recognition commits"),
        bus::CommitOutcome::Committed(_)
    ));
    Fixture {
        bus,
        projects: projects(),
        authority,
    }
}

/// Run the armed renewal at `at` rather than at wall time.
fn renew_at(timers: &mut Runtime, fixture: &Fixture, at: Instant) -> Fired {
    timers
        .synchronize(&fixture.bus)
        .expect("vacant claim with a running job arms");
    let authority = timers
        .renewals
        .get(&fixture.authority.task)
        .expect("vacated renewal is armed")
        .authority
        .clone();
    timers
        .renew(&fixture.bus, authority, at)
        .expect("renewal consumes")
}
