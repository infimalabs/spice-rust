//! The keeper, exercised in a thread against a real shell.

use super::*;
use crate::resident::lane_surface::seed_task_claim;
use bus::Bus;
use journal::{FactCause, Journal, JournalWrite};
use wire::{ClaimDisposition, LaneResidency, Producer, TaskStamp};

/// How long a test waits for a job to answer before it fails.
const TEST_BOUND: Duration = Duration::from_secs(2);

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "spice-job-{name}-{}-{}",
        std::process::id(),
        now_millis().expect("fixture time")
    ));
    fs::create_dir_all(&root).expect("scratch directory");
    root
}

fn argv(command: &str) -> Vec<OsString> {
    ["/bin/sh", "-c", command].map(OsString::from).to_vec()
}

fn claimed_bus() -> (Bus, ServiceClaim, TaskStamp) {
    let bus =
        Bus::start(Journal::in_memory().expect("fixture journal")).expect("fixture bus starts");
    let lane = uuid(1);
    let occupant = uuid(2);
    let epoch = uuid(3);
    let row = LaneResidency {
        lane,
        team: lane,
        name: Some("claimant".to_owned()),
        worktree: "/fixture".to_owned(),
        life: "steer".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: Vec::new(),
        session: None,
        disposition: "occupied".to_owned(),
    };
    let task: TaskStamp = "1kKRRsDP".parse().expect("fixture task");
    let start = "2026-10-09T06:00:00.000Z".parse().expect("fixture start");
    let expiry = "2030-10-09T07:00:00.000Z".parse().expect("fixture expiry");
    let maximum = "2030-10-09T08:00:00.000Z".parse().expect("fixture maximum");
    seed_task_claim(&bus, &task, &row, epoch, start, expiry, maximum);
    let snapshot = bus.task_snapshot().expect("claim snapshot reads");
    let claim = snapshot.tasks()[0].claim().expect("claim head").body();
    let boundary = ServiceClaim {
        task: task.clone(),
        claimed: claim.claimed,
        phase: snapshot.tasks()[0]
            .workflow()
            .body()
            .phase
            .as_str()
            .to_owned(),
        lane,
        occupant,
    };
    (bus, boundary, task)
}

fn release(bus: &Bus, task: &TaskStamp, disposition: ClaimDisposition) {
    let snapshot = bus.task_snapshot().expect("claim snapshot reads");
    let mut claim = snapshot.tasks()[0]
        .claim()
        .expect("claim head")
        .body()
        .clone();
    let at = "2026-10-09T06:30:00.000Z"
        .parse()
        .expect("fixture release time");
    let origin = format!("task:{task}").parse().expect("fixture origin");
    let actor = claim.actor.clone();
    let producer = match disposition {
        ClaimDisposition::Lapsed => Producer::BusTimer,
        ClaimDisposition::Freed => Producer::CommandDoor,
        ClaimDisposition::Active => panic!("active is not a release"),
    };
    let mut write = JournalWrite::new(actor.clone(), origin, at, producer);
    let release = match disposition {
        ClaimDisposition::Freed => write
            .fact(
                wire::body::ClaimFreed {
                    task: task.clone(),
                    actor,
                    parked: None,
                },
                FactCause::Root,
            )
            .expect("claim release fact"),
        ClaimDisposition::Lapsed => write
            .fact(
                wire::body::ClaimLapsed {
                    task: task.clone(),
                    occupant: claim.occupant,
                    expiry: at,
                },
                FactCause::Root,
            )
            .expect("claim lapse fact"),
        ClaimDisposition::Active => panic!("active is not a release"),
    };
    write.claim_head(task.clone(), release, move |position| {
        claim.fact = position;
        claim.disposition = disposition;
        claim
    });
    bus.commit(write).expect("claim release commits");
}

fn carry(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus.task_snapshot().expect("claim snapshot reads");
    let heads = &snapshot.tasks()[0];
    let mut claim = heads.claim().expect("claim head").body().clone();
    let mut lease = heads.lease().expect("lease head").body().clone();
    let occupant = uuid(9);
    let epoch = uuid(10);
    let at = "2026-10-09T06:15:00.000Z"
        .parse()
        .expect("fixture carry time");
    let origin = format!("resident:{epoch}").parse().expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    let carried = write
        .fact(
            wire::body::ClaimCarried {
                task: task.clone(),
                lane: claim.lane,
                occupant,
                ancestor: claim.occupant,
                epoch,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("claim carry fact");
    claim.occupant = occupant;
    claim.epoch = epoch;
    write.claim_head(task.clone(), carried, move |position| {
        claim.fact = position;
        claim
    });
    lease.epoch = epoch;
    write.lease_head(task.clone(), carried, move |_| lease);
    bus.commit(write).expect("claim carry commits");
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

#[test]
fn an_ending_is_unheard_until_the_delivery_record_appears() {
    let root = scratch("unheard");
    assert!(unheard(&root, Duration::ZERO));
    let heard = thread::spawn({
        let root = root.clone();
        move || unheard(&root, TEST_BOUND)
    });
    File::create(root.join(DELIVERED)).expect("the record");
    assert!(!heard.join().expect("the wait ends"));
    assert!(!unheard(&root, Duration::ZERO));
    let _ = fs::remove_dir_all(root);
}

/// A lane's row lists what it has yet to hear without opening the record of
/// anything delivered, so a delivered job whose record is gone or garbled
/// neither appears nor fails the reading.
#[test]
fn kept_jobs_are_read_without_opening_delivered_records() {
    let root = scratch("kept");
    let jobs = Jobs::new(root.join("jobs"));
    let job = jobs
        .start(
            "kept",
            argv("exit 0"),
            &root,
            &Environment::empty(),
            None,
            None,
            &[],
        )
        .expect("the job starts");
    let delivered = root.join("jobs").join("6ZbQk3mPx");
    fs::create_dir_all(&delivered).expect("a delivered job's directory");
    fs::write(delivered.join(REQUEST), b"{garbled").expect("a record beyond reading");
    File::create(delivered.join(DELIVERED)).expect("its delivery");
    assert_eq!(
        jobs.kept()
            .expect("kept jobs read")
            .iter()
            .map(Job::id)
            .collect::<Vec<_>>(),
        [job.id()]
    );
    assert_eq!(
        jobs.delivered(None).expect("delivered names read"),
        ["6ZbQk3mPx"],
        "history names a delivered job without opening its record"
    );
    // The job is still writing its ending; let it finish before its root goes.
    let Observed::Ended(_) = job
        .observe(&Control::new(), TEST_BOUND)
        .expect("the job is observed")
    else {
        panic!("the job ends");
    };
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_job_runs_to_its_end_and_publishes_how_it_ended() {
    let root = scratch("ends");
    let jobs = Jobs::new(root.join("jobs"));
    let job = jobs
        .start(
            "say",
            argv("printf 'out\\n'; printf 'err\\n' >&2; exit 3"),
            &root,
            &Environment::empty(),
            Some(Duration::from_secs(5)),
            None,
            &["noted".to_owned()],
        )
        .expect("the job starts");
    let control = Control::new();
    let Observed::Ended(ended) = job
        .observe(&control, TEST_BOUND)
        .expect("the job is observed")
    else {
        panic!("the job ends");
    };
    assert_eq!(ended.ending, Ending::Exited { code: 3 });
    assert!(ended.wall.is_some());
    assert_eq!(
        job.tail().expect("the log reads"),
        Tail {
            text: "out\nerr\n".to_owned(),
            truncated: false
        }
    );
    let kept = jobs.kept().expect("the lane's kept jobs read");
    assert_eq!(kept.len(), 1, "an unheard ending is still owed");
    assert_eq!(kept[0].id(), job.id());
    assert_eq!(kept[0].command(), "say");
    let reopened = jobs.open(job.id()).expect("the job reopens");
    assert_eq!(reopened.command(), "say");
    assert_eq!(reopened.timeout(), Some(Duration::from_secs(5)));
    assert_eq!(reopened.notices(), ["noted".to_owned()]);
    assert_eq!(reopened.ended(), Some(ended));
    assert!(Key::shaped(job.id()), "{} is a key", job.id());
    assert!(jobs.open("../jobs").is_err());
    assert!(jobs.open("0badcafe").is_err());
    assert!(jobs.open("1kJK6N3Rb").is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_stopped_job_is_ended_and_an_interrupted_observation_yields() {
    let root = scratch("stops");
    let jobs = Jobs::new(root.join("jobs"));
    // A fifo nobody writes holds its reader open, so the command ends
    // only because the keeper ends it.
    let status = ProcessCommand::new("mkfifo")
        .arg(root.join("hold"))
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let job = jobs
        .start(
            "hold",
            argv("cat hold"),
            &root,
            &Environment::empty(),
            None,
            None,
            &[],
        )
        .expect("the job starts");
    let control = Control::new();
    control.handle().interrupt();
    assert_eq!(
        job.observe(&control, TEST_BOUND)
            .expect("the job is observed"),
        Observed::Interrupted
    );
    assert_eq!(
        job.observe(&control, Duration::ZERO)
            .expect("the job is observed"),
        Observed::Elapsed
    );
    job.stop().expect("the stop is asked");
    let Observed::Ended(ended) = job
        .observe(&control, TEST_BOUND)
        .expect("the job is observed")
    else {
        panic!("the job ends");
    };
    assert_eq!(ended.ending, Ending::Stopped);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_job_past_its_deadline_is_ended_as_timed_out() {
    let root = scratch("deadline");
    let jobs = Jobs::new(root.join("jobs"));
    let status = ProcessCommand::new("mkfifo")
        .arg(root.join("hold"))
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let job = jobs
        .start(
            "hold",
            argv("cat hold"),
            &root,
            &Environment::empty(),
            Some(Duration::from_millis(20)),
            None,
            &[],
        )
        .expect("the job starts");
    let Observed::Ended(ended) = job
        .observe(&Control::new(), TEST_BOUND)
        .expect("the job is observed")
    else {
        panic!("the job ends");
    };
    assert_eq!(ended.ending, Ending::TimedOut);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_command_that_cannot_run_ends_as_failed() {
    let root = scratch("fails");
    let jobs = Jobs::new(root.join("jobs"));
    let job = jobs
        .start(
            "nothing",
            vec![OsString::from("/nonexistent/spice-job-shell")],
            &root,
            &Environment::empty(),
            Some(Duration::from_secs(5)),
            None,
            &[],
        )
        .expect("the job starts");
    let observed = job
        .observe(&Control::new(), TEST_BOUND)
        .expect("the job is observed");
    assert!(
        matches!(
            observed,
            Observed::Ended(Ended {
                ending: Ending::Failed { .. },
                ..
            })
        ),
        "{observed:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn services_stop_when_their_claim_is_freed_or_lapsed() {
    for disposition in [ClaimDisposition::Freed, ClaimDisposition::Lapsed] {
        let root = scratch(match disposition {
            ClaimDisposition::Freed => "service-freed",
            ClaimDisposition::Lapsed => "service-lapsed",
            ClaimDisposition::Active => unreachable!(),
        });
        let jobs = Jobs::new(root.join("jobs"));
        let (bus, boundary, task) = claimed_bus();
        let job = jobs
            .start(
                "service",
                argv("sleep 30"),
                &root,
                &Environment::empty(),
                None,
                Some(boundary),
                &[],
            )
            .expect("service starts");

        jobs.reconcile_services(
            &bus.task_snapshot().expect("active snapshot reads"),
            &mut io::sink(),
        );
        assert_eq!(job.ended(), None, "an active claim retains its service");

        release(&bus, &task, disposition);
        jobs.reconcile_services(
            &bus.task_snapshot().expect("released snapshot reads"),
            &mut io::sink(),
        );
        assert!(matches!(
            job.observe(&Control::new(), TEST_BOUND)
                .expect("stopped service is observed"),
            Observed::Ended(Ended {
                ending: Ending::Stopped,
                ..
            })
        ));
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn claim_reconciliation_never_stops_a_finite_job() {
    let root = scratch("finite-unbound");
    let jobs = Jobs::new(root.join("jobs"));
    let job = jobs
        .start(
            "finite",
            argv("sleep 30"),
            &root,
            &Environment::empty(),
            Some(Duration::from_secs(30)),
            None,
            &[],
        )
        .expect("finite job starts");
    let bus =
        Bus::start(Journal::in_memory().expect("fixture journal")).expect("fixture bus starts");

    jobs.reconcile_services(
        &bus.task_snapshot().expect("empty snapshot reads"),
        &mut io::sink(),
    );

    assert_eq!(job.ended(), None);
    job.stop().expect("finite job stops");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_carried_claim_keeps_its_service_running() {
    let root = scratch("service-carried");
    let jobs = Jobs::new(root.join("jobs"));
    let (bus, boundary, task) = claimed_bus();
    let job = jobs
        .start(
            "service",
            argv("sleep 30"),
            &root,
            &Environment::empty(),
            None,
            Some(boundary),
            &[],
        )
        .expect("service starts");

    carry(&bus, &task);
    jobs.reconcile_services(
        &bus.task_snapshot().expect("carried snapshot reads"),
        &mut io::sink(),
    );

    assert_eq!(job.ended(), None);
    job.stop().expect("service stops");
    let _ = fs::remove_dir_all(root);
}
