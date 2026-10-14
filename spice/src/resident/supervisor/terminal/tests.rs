use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sill::Environment;
use vendor::omp::{JobCall, JobOp};
use wire::intent::{LaneJobKind, LaneJobStanding};

use super::job::Ending;
use super::{Control, Jobs, Terminal, unheard, wait};

fn scratch(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-lane-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ))
}

fn append(log: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(log)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .expect("job output appends");
}

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

#[test]
fn a_finite_wait_reports_what_its_log_gains_though_its_directory_never_changes() {
    let root = scratch("wait-refresh");
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let job = jobs.record_for_tests("serve --port 8080", false, Some(600_000));
    fs::write(job.log_path(), "building\n").expect("job output");
    let control = Control::new();
    let handle = control.handle();
    let mut progress = Vec::new();

    wait(
        &job,
        &control,
        Duration::from_secs(30),
        Duration::from_millis(10),
        &mut |report| {
            progress.push(report.output.clone());
            if progress.len() == 1 {
                append(&job.log_path(), "ready on 8080\n");
            } else {
                handle.cancel();
            }
        },
    );

    let _ = fs::remove_dir_all(&root);
    let named = format!("[job {}] serve --port 8080\n", job.id());
    assert_eq!(
        progress,
        [named.clone() + "building", named + "ready on 8080"]
    );
}

#[test]
fn a_finite_wait_reports_at_most_once_per_cadence_however_often_its_job_changes() {
    let root = scratch("wait-cadence");
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let job = jobs.record_for_tests("chatty", false, Some(600_000));
    fs::write(job.log_path(), "0\n").expect("job output");
    let cadence = Duration::from_millis(20);
    let stopped = Arc::new(AtomicBool::new(false));
    let writer = {
        let (log, stopped) = (job.log_path().to_path_buf(), Arc::clone(&stopped));
        let probe = log.with_file_name("probe");
        std::thread::spawn(move || {
            let mut line = 0_u64;
            while !stopped.load(Ordering::Relaxed) {
                line += 1;
                append(&log, &format!("{line}\n"));
                // A directory entry change wakes the watch on every platform.
                let _ = fs::write(&probe, b"");
                let _ = fs::remove_file(&probe);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let mut reported = Vec::new();

    wait(
        &job,
        &Control::new(),
        Duration::from_millis(200),
        cadence,
        &mut |_| reported.push(Instant::now()),
    );

    stopped.store(true, Ordering::Relaxed);
    writer.join().expect("writer stops");
    let _ = fs::remove_dir_all(&root);
    assert!(
        reported.len() > 1,
        "a job that keeps printing is reported again"
    );
    // The wait spaces its reads by the cadence; each report follows its read
    // by however long that read took, so reports keep at least half of it.
    assert!(
        reported
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= cadence / 2),
        "{reported:?}"
    );
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
