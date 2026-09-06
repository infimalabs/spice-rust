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

fn crash_large_record_publication(fixture: &Fixture) -> PathBuf {
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
    temporary.clone()
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
    assert!(reason.contains(
        "reclaim through its admitted current direction: preview plus exact digest-bound apply, or `spice install --unapply` for an in-progress reversal"
    ));
    assert!(reason.contains("receipt diagnosis: not-applicable: installation receipt is absent"));
    assert_eq!(temporary_snapshot(temporary), aged_snapshot);
    aged
}

fn preview_then_reclaim_publication(fixture: &Fixture, temporary: &Path, aged: u64) {
    let before = temporary_snapshot(temporary);
    let preview: InstallPreview = reply_body(invoke_at(fixture, &["--json", "install"], aged));
    assert_eq!(
        temporary_snapshot(temporary),
        before,
        "preview must be read-only"
    );
    let moved = alternate_digest(preview.digest().as_str());
    let refused = invoke_at(
        fixture,
        &["--json", "install", &format!("--apply={moved}")],
        aged,
    );
    assert_eq!(refused.code, Some(2));
    assert_refusal(&refused.err, "install::digest-moved");
    assert_eq!(
        temporary_snapshot(temporary),
        before,
        "a refused digest must not admit reclamation"
    );
    let _: InstallReceipt = reply_body(invoke_at(
        fixture,
        &[
            "--json",
            "install",
            &format!("--apply={}", preview.digest()),
        ],
        aged,
    ));
    assert!(record_temps(fixture).is_empty());
}

#[test]
fn killed_record_publication_is_reported_then_reclaimed() {
    let fixture = Fixture::new("crashed-record-publication");
    let temporary = crash_large_record_publication(&fixture);
    let aged = age_and_diagnose_publication(&fixture, &temporary);
    preview_then_reclaim_publication(&fixture, &temporary, aged);
}

fn fold_path(fixture: &Fixture) -> PathBuf {
    fixture.root.join(".git/.spice-rs/data/spicefolds.sqlite3")
}

fn crash_fold_retirement(fixture: &Fixture, action: &str) -> (String, PathBuf, PathBuf, u64) {
    let fold = fold_path(fixture);
    fs::write(&fold, b"crash-only fold payload\n").expect("write obsolete fold payload");
    let inode = fs::metadata(&fold)
        .expect("inspect obsolete fold payload")
        .ino();
    let unrelated = fold.parent().unwrap().join(".spice-install-unrelated.tmp");
    fs::write(&unrelated, b"operator temporary\n").expect("write unrelated temporary");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert!(preview.plan().rollback_archive().is_none());
    let (index, retirement) = preview
        .plan()
        .retirements()
        .iter()
        .enumerate()
        .find(|(_, retirement)| retirement.path() == ".spice-rs/data/spicefolds.sqlite3")
        .expect("preview carries the fold retirement");
    assert!(retirement.rollback().is_none());
    let temporary = fold
        .parent()
        .unwrap()
        .join(format!(".spice-install-retirement-{index}.tmp"));
    let digest = preview.digest().to_string();
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        action,
        fold.clone(),
        Release::Withheld,
    );
    let (_, killed) = kill(paused);
    assert_eq!(killed.code, None);
    (digest, temporary, unrelated, inode)
}

fn resume_fold_retirement(fixture: &Fixture, digest: &str) {
    let receipt: InstallReceipt =
        reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));
    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert!(fs::metadata(fixture.record()).unwrap().len() < MIN_PRIVATE_INSTALL_RECORD as u64);
}

#[test]
fn killed_after_fold_rename_completes_only_the_admitted_retirement() {
    let fixture = Fixture::new("fold-retirement-renamed");
    let fold = fold_path(&fixture);
    let (digest, temporary, unrelated, inode) = crash_fold_retirement(&fixture, "retire-renamed");
    assert!(!fold.exists());
    assert_eq!(
        fs::metadata(&temporary)
            .expect("renamed fold payload remains")
            .ino(),
        inode,
    );

    resume_fold_retirement(&fixture, &digest);

    assert!(!fold.exists());
    assert!(!temporary.exists());
    assert_eq!(fs::read(&unrelated).unwrap(), b"operator temporary\n");
    assert!(
        !fs::read(fixture.record())
            .unwrap()
            .windows(b"crash-only fold payload".len())
            .any(|window| window == b"crash-only fold payload")
    );
}

#[test]
fn killed_after_fold_unlink_recognizes_and_records_completion() {
    let fixture = Fixture::new("fold-retirement-unlinked");
    let fold = fold_path(&fixture);
    let (digest, temporary, unrelated, _) = crash_fold_retirement(&fixture, "retire-unlinked");
    assert!(!fold.exists());
    assert!(!temporary.exists());

    resume_fold_retirement(&fixture, &digest);

    assert!(!fold.exists());
    assert!(!temporary.exists());
    assert_eq!(fs::read(&unrelated).unwrap(), b"operator temporary\n");
    assert!(
        !fs::read(fixture.record())
            .unwrap()
            .windows(b"crash-only fold payload".len())
            .any(|window| window == b"crash-only fold payload")
    );
}
