//! Renewal of a claim whose occupant ended its turn on a kept job.

use std::ffi::OsString;
use std::time::Duration;

use sill::Environment;

use super::super::super::supervisor::terminal::{Job, Jobs};
use super::*;

/// How long a test waits for a keeper to publish its state.
const KEEPER_BOUND: Duration = Duration::from_secs(2);

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
fn a_vacant_lane_never_rolls_its_claim_past_the_job_deadline_and_delivery() {
    let fixture = automatic_fixture();
    let supervisors = Supervisors::new("deadline");
    let timeout = Duration::from_secs(60);
    let job = supervisors.start(fixture.authority.lane, "sleep 30", Some(timeout));
    let started = job
        .state()
        .expect("job state reads")
        .expect("keeper published")
        .started_millis;
    vacate(&fixture);
    let mut timers = supervisors.timers(&fixture);
    timers
        .synchronize(&fixture.bus)
        .expect("vacant claim with a running job arms");

    assert_eq!(renew_once(&mut timers, &fixture), Fired::ClaimRenew);
    // Keeper start, timeout, one second to terminate, two to deliver.
    let delivered = started + 60_000 + 1_000 + 2_000;
    let bound = Instant::new(
        jiff::Timestamp::from_millisecond(i64::try_from(delivered).expect("fixture millis"))
            .expect("fixture bound"),
    )
    .expect("fixture bound is canonical");
    let renewed = lease(&fixture);
    assert!(
        shifted(
            renewed.renewal,
            crate::board_command::AUTOMATIC_ROLLING_SECONDS
        ) > bound
    );
    assert_eq!(renewed.expiry, bound);
    timers
        .synchronize(&fixture.bus)
        .expect("bounded claim folds");
    assert!(
        timers.renewals.is_empty(),
        "a lease already reaching the job's delivery is not renewed again",
    );

    job.stop().expect("the job stops");
    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}
