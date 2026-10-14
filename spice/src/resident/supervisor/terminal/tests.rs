use std::fs;
use std::time::Duration;

use sill::Environment;
use vendor::omp::{JobCall, JobOp};
use wire::intent::{LaneJobKind, LaneJobStanding};

use super::job::Ending;
use super::{Control, Jobs, Terminal, unheard};

#[test]
fn a_lane_lists_each_kept_job_by_how_it_lives_and_whether_it_ended() {
    let root = std::env::temp_dir().join(format!(
        "spice-lane-jobs-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ));
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let command = jobs.record_for_tests("cargo test", false, Some(600_000));
    let service = jobs.record_for_tests("serve --port 8080", false, None);
    let watch = jobs.record_for_tests("tail -f build.log", true, Some(600_000));
    command.end_for_tests(Ending::Exited { code: 1 });

    let mut listed = unheard(&jobs)
        .into_iter()
        .map(|job| (job.id, job.kind, job.standing))
        .collect::<Vec<_>>();
    listed.sort();
    let mut expected = vec![
        (
            command.id().to_owned(),
            LaneJobKind::Command,
            LaneJobStanding::Unheard,
        ),
        (
            service.id().to_owned(),
            LaneJobKind::Service,
            LaneJobStanding::Running,
        ),
        (
            watch.id().to_owned(),
            LaneJobKind::Watch,
            LaneJobStanding::Running,
        ),
    ];
    expected.sort();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(listed, expected);
}

#[test]
fn a_finite_wait_reports_the_job_and_latest_output_while_it_remains_blocked() {
    let root = std::env::temp_dir().join(format!(
        "spice-lane-wait-progress-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ));
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let job = jobs.record_for_tests("serve --port 8080", false, Some(600_000));
    fs::write(job.log_path(), "building\nready on 8080\n").expect("job output");
    let terminal = Terminal::new("/bin/sh", &root, Environment::empty(), jobs);
    let call = JobCall {
        op: JobOp::Wait,
        id: Some(job.id().to_owned()),
        timeout: Duration::ZERO,
    };
    let mut progress = Vec::new();

    let report = terminal.job(&call, &Control::new(), &mut |report| {
        progress.push(report.output.clone());
    });

    let _ = fs::remove_dir_all(&root);
    assert_eq!(
        progress,
        ["[job ".to_owned() + job.id() + "] serve --port 8080\nready on 8080"]
    );
    assert!(report.output.contains("Command still running"));
}

/// A confirmed no-process launch failure removes its record. Cleanup failures
/// and unknown process ownership preserve the record and remain observable
/// rather than deleting a possibly live keeper or masquerading as a clean refusal.
#[test]
fn a_job_whose_keeper_cannot_launch_is_never_silently_kept() {
    let root = std::env::temp_dir().join(format!(
        "spice-lane-jobs-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ));
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let (failure, record_exists) = jobs.start_unlaunchable_for_tests("echo alive");
    let kept = jobs.kept().expect("kept jobs read");
    assert_eq!(failure.kind(), std::io::ErrorKind::NotFound);
    assert!(!record_exists, "a failed launch left its job directory");
    assert!(
        kept.is_empty(),
        "a failed launch left kept jobs: {:?}",
        kept.iter().map(super::Job::id).collect::<Vec<_>>()
    );

    let (failure, record_exists) = jobs.start_uncleanable_for_tests("echo stranded");
    let kept = jobs.kept().expect("uncleaned jobs read");
    assert_eq!(failure.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(record_exists, "the cleanup fixture removed its record");
    assert_eq!(kept.len(), 1, "the surviving record must remain visible");

    let (failure, record_exists) = jobs.start_unknown_for_tests("echo possibly-alive");
    let kept = jobs.kept().expect("unknown-ownership jobs read");
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(failure.kind(), std::io::ErrorKind::BrokenPipe);
    assert!(
        record_exists,
        "unknown process ownership deleted its record"
    );
    assert_eq!(kept.len(), 2, "both preserved records must remain visible");
}
