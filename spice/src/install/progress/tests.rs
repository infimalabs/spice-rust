use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::super::{RECORD_PATH, SKILL_PATH};
use super::*;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository(label: &str) -> (Scratch, Repository) {
    let root = std::env::temp_dir().join(format!(
        "spice-install-progress-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&root).expect("scratch root creates");
    crate::test_repository::initialize(&root);
    let repository = Repository::discover(&root).expect("scratch repository discovers");
    (Scratch(root), repository)
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().expect("fixture file has a parent"))
        .expect("fixture parent creates");
    fs::write(path, bytes).expect("fixture bytes write");
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode writes");
    #[cfg(windows)]
    sill::authority::set_owner_mode(path, mode).expect("fixture mode writes");
}

fn mode_of(path: &Path) -> u32 {
    let file = fs::File::open(path).expect("fixture reopens");
    sill::authority::file_mode(&file).expect("fixture mode reads") & 0o777
}

#[test]
fn preserved_payloads_stay_out_of_checkpoints_and_survive_both_resume_frontiers() {
    let (_scratch, repository) = repository("reversal-payload");
    let target = repository.common_dir().join(SKILL_PATH);
    let prior = vec![0xA5; 2 * 1024 * 1024];
    // Windows restores only owner-only modes, so that is all it admits.
    let prior_mode = if cfg!(unix) { 0o640 } else { 0o600 };
    write_mode(&target, &prior, prior_mode);

    // Start a real apply and stop after the preserved target moved but
    // before its completion reached the receipt: the one admitted
    // ahead-of-record crash frontier.
    let prepared = prepare(&repository, None).expect("install prepares");
    let PreparedInstall {
        mut record,
        receipt,
        fresh,
    } = prepared;
    assert!(fresh);
    let generated = materialize_operations(&record, None, None).expect("operations materialize");
    record
        .plan
        .validate_pristine(&repository)
        .expect("pristine plan validates");
    require_receipt(&repository, &record.plan, receipt.as_ref()).expect("receipt remains absent");
    let mut generation =
        write_initial_record(&repository, &record).expect("initial receipt publishes");
    reversal::ensure_payloads(&repository, &record.plan).expect("reversal payloads publish");
    let skill = record
        .plan
        .operations
        .iter()
        .position(|operation| operation.asset == Asset::Skill)
        .expect("skill operation exists");
    let mut durable_completed = record.completed;
    while record.completed < skill {
        let index = record.completed;
        let (next, changed) = apply_operation(
            &repository,
            &mut record,
            &generated[index],
            index,
            generation,
            durable_completed,
        )
        .expect("operation applies");
        generation = next;
        record.completed += 1;
        if changed {
            generation =
                write_record(&repository, &record, generation).expect("progress publishes");
            durable_completed = record.completed;
        }
    }
    let (_, changed) = apply_operation(
        &repository,
        &mut record,
        &generated[skill],
        skill,
        generation,
        durable_completed,
    )
    .expect("frontier operation applies");
    assert!(changed);
    drop(record);

    let resumed = prepare(&repository, None).expect("interrupted apply reloads");
    apply_prepared_record(&repository, None, resumed).expect("interrupted apply completes");
    let record_length = fs::metadata(repository.common_dir().join(RECORD_PATH))
        .expect("receipt metadata reads")
        .len();
    assert!(
        record_length < u64::try_from(prior.len()).unwrap(),
        "the receipt embedded the preserved payload"
    );

    // Enter unapply and stop after one real file-reversal checkpoint. The
    // next invocation must accept that durable prefix and finish from the
    // external payload store.
    let LoadedRecord {
        mut record,
        generation,
    } = read_record(&repository)
        .expect("receipt reads")
        .expect("receipt exists");
    verify_progress(&repository, &record).expect("completed install verifies");
    reversal::ensure_payloads(&repository, &record.plan).expect("payload store verifies");
    record.status = InstallStatus::Unapplying;
    record.completed = 0;
    record.restored.fill(None);
    record.restored_retirements.fill(None);
    let mut generation = write_record(&repository, &record, generation).expect("unapply starts");
    while let Some(next) = reverse_retirement(&repository, &mut record, generation.clone())
        .expect("retirement reverses")
    {
        generation = next;
    }
    reverse_file_operation(&repository, &mut record, generation)
        .expect("one file reversal publishes");
    drop(record);

    let receipt = unapply_record(&repository).expect("interrupted unapply completes");
    assert_eq!(receipt.status(), InstallStatus::Uninstalled);
    assert_eq!(fs::read(&target).expect("prior bytes read"), prior);
    assert_eq!(mode_of(&target), prior_mode);
}
