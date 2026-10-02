//! The keeper, exercised in a thread against a real shell.

use super::*;

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
