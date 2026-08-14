use super::super::record::InstallRecord;
use super::*;

struct MigrationFixture {
    scratch: Scratch,
    repository: Repository,
    binary: PathBuf,
    prior: InstallRecord,
    prior_record: Vec<u8>,
    prior_assets: Vec<Option<ObservedFile>>,
    prior_hooks: Vec<String>,
    legacy_xtask: PathBuf,
    legacy_guard: PathBuf,
    xtask_bytes: Vec<u8>,
    guard_bytes: Vec<u8>,
}

#[test]
fn version_7_upgrade_retires_legacy_runtime_after_publication_and_rolls_back_exactly() {
    let fixture = version_7_fixture();
    let migration = preview_migration(&fixture);
    interrupt_after_first_retirement(&fixture, &migration);
    resume_and_reverse(fixture, &migration);
}

fn version_7_fixture() -> MigrationFixture {
    let scratch = Scratch::new("version-7-migration");
    let repository = scratch.repository();
    let binary = scratch.binary(&vec![b's'; LARGE_BINARY_BYTES]);
    let initial = plan(&repository, Some(&binary)).expect("plan fixture appliance");
    apply(&repository, Some(&binary), &initial.digest().unwrap())
        .expect("install fixture appliance");
    let (prior, prior_record) = publish_version_7_receipt(&repository);

    let legacy_xtask = repository.common_dir().join(RETIRED_XTASK_PATH);
    let legacy_guard = repository.common_dir().join(RETIRED_XTASK_GUARD_PATH);
    let xtask_bytes = vec![b'x'; LARGE_BINARY_BYTES];
    let guard_bytes = b"0123456789abcdef\n".to_vec();
    fs::write(&legacy_xtask, &xtask_bytes).expect("publish legacy xtask");
    set_test_mode(&legacy_xtask, EXECUTABLE_MODE);
    fs::write(&legacy_guard, &guard_bytes).expect("publish legacy guard stamp");
    set_test_mode(&legacy_guard, DATA_MODE);
    let prior_assets = images(&repository, &prior.plan);
    let prior_hooks = git_values(
        scratch.path(),
        &["config", "--local", "--get-all", "core.hooksPath"],
    );
    let stale = audit_appliance(&repository, &binary).expect("audit version-7 appliance");
    assert_eq!(stale.state(), ApplianceState::StaleReceipt);
    assert!(stale.detail().contains("forward migration"), "{stale:?}");
    MigrationFixture {
        scratch,
        repository,
        binary,
        prior,
        prior_record,
        prior_assets,
        prior_hooks,
        legacy_xtask,
        legacy_guard,
        xtask_bytes,
        guard_bytes,
    }
}

fn publish_version_7_receipt(repository: &Repository) -> (InstallRecord, Vec<u8>) {
    let mut prior = read_record(repository)
        .unwrap()
        .expect("fixture receipt exists")
        .record;
    prior.version = MIGRATABLE_RECORD_VERSION;
    prior.plan.version = MIGRATABLE_RECORD_VERSION;
    prior.plan.retirements.clear();
    prior.plan.rollback_archive = None;
    prior.retired_parents.clear();
    prior.restored_retirements.clear();
    prior.completed = prior.plan.operation_count();
    prior.plan_digest = prior.plan.digest().expect("version-7 plan has a digest");
    let bytes = wire::encode(&prior).expect("version-7 receipt encodes");
    fs::write(record_path(repository).unwrap(), &bytes).expect("publish version-7 receipt");
    set_test_mode(&record_path(repository).unwrap(), RECORD_MODE);
    (prior, bytes)
}

fn preview_migration(fixture: &MigrationFixture) -> InstallPlan {
    let migration =
        plan(&fixture.repository, Some(&fixture.binary)).expect("derive version-8 migration");
    assert_eq!(migration.version(), PLAN_VERSION);
    assert_eq!(
        migration
            .retirements()
            .iter()
            .map(Retirement::path)
            .collect::<Vec<_>>(),
        [RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH],
    );
    assert!(migration.has_rollback_archive());
    assert!(migration.canonical_bytes().unwrap().len() < MAX_COMPACT_PLAN_BYTES);
    let rollback = migration
        .rollback_state()
        .expect("decode migration rollback")
        .expect("migration owns rollback state");
    assert_eq!(rollback.record.bytes(), fixture.prior_record);
    assert_eq!(
        rollback.operations,
        fixture
            .prior_assets
            .iter()
            .map(|observed| observed.as_ref().map(|observed| observed.image.clone()))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        rollback.retirements,
        [
            Some(FileImage::new(EXECUTABLE_MODE, fixture.xtask_bytes.clone())),
            Some(FileImage::new(DATA_MODE, fixture.guard_bytes.clone())),
        ]
    );
    migration
}

fn interrupt_after_first_retirement(fixture: &MigrationFixture, migration: &InstallPlan) {
    let digest = migration.digest().unwrap();
    let first_retirement = migration.operations().len();
    let interrupted = apply_with_interruption(
        &fixture.repository,
        Some(&fixture.binary),
        &digest,
        Some(first_retirement),
    )
    .expect_err("interrupt after first retirement");
    assert!(matches!(
        interrupted,
        InstallError::Interrupted { operation } if operation == first_retirement
    ));
    assert!(!fixture.legacy_xtask.exists());
    assert!(fixture.legacy_guard.exists());
    assert_eq!(
        git_values(
            fixture.scratch.path(),
            &["config", "--local", "--get-all", "core.hooksPath"],
        ),
        fixture.prior_hooks,
    );
    for operation in migration.operations() {
        let observed = observe(&operation.destination(&fixture.repository)).unwrap();
        assert!(matches_generated(observed.as_ref(), operation));
    }
}

fn resume_and_reverse(fixture: MigrationFixture, migration: &InstallPlan) {
    let digest = migration.digest().unwrap();
    let migrated =
        apply(&fixture.repository, Some(&fixture.binary), &digest).expect("resume migration");
    assert_eq!(migrated.status(), InstallStatus::Complete);
    assert!(!fixture.legacy_xtask.exists());
    assert!(!fixture.legacy_guard.exists());
    assert!(
        audit_appliance(&fixture.repository, &fixture.binary)
            .expect("audit migrated appliance")
            .is_current()
    );

    let rolled_back = unapply(&fixture.repository).expect("reverse migration");
    assert_eq!(rolled_back.status(), InstallStatus::Uninstalled);
    assert_eq!(
        fs::read(&fixture.legacy_xtask).unwrap(),
        fixture.xtask_bytes
    );
    assert_eq!(mode(&fixture.legacy_xtask), EXECUTABLE_MODE);
    assert_eq!(
        fs::read(&fixture.legacy_guard).unwrap(),
        fixture.guard_bytes
    );
    assert_eq!(mode(&fixture.legacy_guard), DATA_MODE);
    assert_eq!(
        images(&fixture.repository, &fixture.prior.plan),
        fixture.prior_assets
    );
    assert_eq!(
        git_values(
            fixture.scratch.path(),
            &["config", "--local", "--get-all", "core.hooksPath"],
        ),
        fixture.prior_hooks,
    );
    let restored = read_record(&fixture.repository)
        .unwrap()
        .expect("version-7 receipt was restored");
    assert_eq!(restored.record.version, MIGRATABLE_RECORD_VERSION);
    assert_eq!(restored.record.status, InstallStatus::Complete);
    assert_eq!(
        restored
            .image
            .expect("version-7 receipt retains its source image")
            .bytes,
        fixture.prior_record
    );
}
