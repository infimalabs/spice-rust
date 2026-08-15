//! Crash-recovery proof for an interrupted install-record publication.

use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wire::{DoctorObservation, DoctorReport, DoctorSubsystem};

use super::*;

#[derive(Debug, Eq, PartialEq)]
struct TemporarySnapshot {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn temporary_snapshot(path: &Path) -> TemporarySnapshot {
    let metadata = fs::metadata(path).expect("inspect temporary record");
    TemporarySnapshot {
        bytes: fs::read(path).expect("read fixture temporary record"),
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn invoke_at(fixture: &Fixture, arguments: &[&str], unix_seconds: u64) -> Spoken {
    let output = fixture
        .command(arguments)
        .env(
            spice::install::TEST_PAUSE_ACTION,
            format!("maintenance-now:{unix_seconds}"),
        )
        .output()
        .expect("spice install launches with test maintenance time");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

fn record_temps(fixture: &Fixture) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(fixture.root.join(".git/.spice-rs"))
        .expect("read runtime directory")
        .map(|entry| entry.expect("read runtime entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".spice-install-") && name.ends_with(".tmp"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn kill(mut paused: Paused) -> (u32, Spoken) {
    let mut child = paused.child.take().expect("paused child is present");
    let pid = child.id();
    child.kill().expect("kill paused installer");
    let output = child
        .wait_with_output()
        .expect("collect killed install process");
    fs::remove_dir_all(&paused.control).expect("remove install race rendezvous");
    (
        pid,
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        },
    )
}

fn installation_observation(spoken: &Spoken) -> DoctorObservation {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<DoctorReport<spice::config::Root>> =
        wire::decode(&spoken.out).expect("Doctor JSON decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("Doctor inspection must reply: {outcome:?}");
    };
    reply
        .body
        .subsystems
        .into_iter()
        .find(|diagnosis| diagnosis.subsystem == DoctorSubsystem::Installation)
        .expect("Doctor reports installation")
        .observation
}

fn crash_large_record_publication(fixture: &Fixture) -> (wire::Digest, PathBuf) {
    let prior_hook = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    fs::create_dir_all(prior_hook.parent().unwrap()).expect("create prior hook parent");
    fs::write(&prior_hook, vec![0x5a; MIN_PRIVATE_INSTALL_RECORD + 1])
        .expect("write large prior hook");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest().clone();
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        fixture.record(),
        Release::Withheld,
    );

    let (pid, killed) = kill(paused);
    assert_eq!(killed.code, None);
    let pid = rustix::process::Pid::from_raw(pid.try_into().unwrap()).unwrap();
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH),
        "killed installer PID must be definitely absent",
    );
    let temporaries = record_temps(fixture);
    let [temporary] = temporaries.as_slice() else {
        panic!(
            "a killed record publication must leave one temporary: {:?}",
            record_temps(fixture)
        );
    };
    assert!(fs::metadata(temporary).unwrap().len() > MIN_PRIVATE_INSTALL_RECORD as u64);
    assert_eq!(
        fs::metadata(temporary).unwrap().permissions().mode() & 0o777,
        0o600
    );
    (digest, temporary.clone())
}

fn assert_fresh_publication_is_inert(fixture: &Fixture, digest: &wire::Digest, temporary: &Path) {
    let before = temporary_snapshot(temporary);
    let immediate_preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(immediate_preview.digest(), digest);
    assert_eq!(temporary_snapshot(temporary), before);
    let immediate = fixture.invoke(&["--json", "doctor"]);
    assert!(matches!(
        installation_observation(&immediate),
        DoctorObservation::NotApplicable { .. }
    ));
    assert_eq!(temporary_snapshot(temporary), before);
}

fn age_and_diagnose_publication(fixture: &Fixture, temporary: &Path) -> u64 {
    let old_modified = SystemTime::now()
        .checked_sub(Duration::from_secs(2 * 60 * 60))
        .expect("wall clock admits an old fixture timestamp");
    File::options()
        .write(true)
        .open(temporary)
        .expect("open temporary record for timestamp aging")
        .set_times(fs::FileTimes::new().set_modified(old_modified))
        .expect("age temporary record through filesystem timestamps");
    let aged_snapshot = temporary_snapshot(temporary);
    let aged = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock follows Unix epoch")
        .as_secs()
        + 2 * 60 * 60;
    let aged_doctor = invoke_at(fixture, &["--json", "doctor"], aged);
    let DoctorObservation::InputRequired { reason } = installation_observation(&aged_doctor) else {
        panic!("aged crashed publication must require input");
    };
    assert!(reason.contains("1 stale install publication file(s)"));
    assert!(reason.contains("run spice install once"));
    assert!(reason.contains("receipt diagnosis: not-applicable: installation receipt is absent"));
    assert_eq!(temporary_snapshot(temporary), aged_snapshot);
    aged
}

fn reclaim_and_resume_installation(fixture: &Fixture, aged: u64) {
    let resumed_preview: InstallPreview =
        reply_body(invoke_at(fixture, &["--json", "install"], aged));
    assert!(record_temps(fixture).is_empty());
    let resumed_digest = resumed_preview.digest();
    let receipt: InstallReceipt = reply_body(invoke_at(
        fixture,
        &["--json", "install", &format!("--apply={resumed_digest}")],
        aged,
    ));
    assert_eq!(receipt.status(), InstallStatus::Complete);

    let text = invoke_at(fixture, &["doctor"], aged);
    text.assert_success();
    assert!(!String::from_utf8_lossy(&text.out).contains("stale install publication"));
    let json = invoke_at(fixture, &["--json", "doctor"], aged);
    assert!(matches!(
        installation_observation(&json),
        DoctorObservation::Ready {}
    ));
}

#[test]
fn killed_large_record_publication_is_reported_then_reclaimed_before_resume() {
    let fixture = Fixture::new("crashed-record-publication");
    let (digest, temporary) = crash_large_record_publication(&fixture);
    assert_fresh_publication_is_inert(&fixture, &digest, &temporary);
    let aged = age_and_diagnose_publication(&fixture, &temporary);
    reclaim_and_resume_installation(&fixture, aged);
}
