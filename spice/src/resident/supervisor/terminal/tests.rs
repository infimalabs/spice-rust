use wire::intent::{LaneJobKind, LaneJobStanding};

use super::job::Ending;
use super::{Jobs, unheard};

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
