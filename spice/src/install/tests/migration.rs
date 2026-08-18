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

#[test]
fn large_rollback_archive_stays_private_to_bounded_public_replies() {
    const PRIVATE_MINIMUM: usize = 1024 * 1024;
    const PUBLIC_MAXIMUM: usize = 64 * 1024;

    let mut fixture = version_7_fixture();
    let compact_migration =
        plan(&fixture.repository, Some(&fixture.binary)).expect("derive compact migration");
    let compact_public = wire::encode(
        &InstallPreview::from_plan(compact_migration).expect("project compact migration"),
    )
    .expect("compact public migration encodes");
    fixture.xtask_bytes = deterministic_bytes(2 * PRIVATE_MINIMUM);
    fs::write(&fixture.legacy_xtask, &fixture.xtask_bytes).expect("publish large legacy xtask");
    set_test_mode(&fixture.legacy_xtask, EXECUTABLE_MODE);

    let migration =
        plan(&fixture.repository, Some(&fixture.binary)).expect("derive large migration");
    let archive = migration
        .rollback_archive
        .as_ref()
        .expect("large migration owns rollback archive");
    let private_plan = migration
        .canonical_bytes()
        .expect("large private migration plan encodes");
    assert!(private_plan.len() < PUBLIC_MAXIMUM);

    let preview = InstallPreview::from_plan(migration.clone()).expect("project large preview");
    let summary = preview
        .plan()
        .rollback_archive()
        .expect("public plan reports rollback metadata");
    assert_eq!(summary.format(), archive.format());
    assert_eq!(summary.uncompressed_bytes(), archive.uncompressed_bytes());
    assert_eq!(summary.digest(), archive.digest());
    assert_public_install_value(&preview, PUBLIC_MAXIMUM);
    assert_eq!(
        wire::encode(&preview)
            .expect("large public migration encodes")
            .len(),
        compact_public.len(),
        "public projection size must not grow with rollback payload size",
    );
    let stage_indices = preview
        .plan()
        .operations()
        .iter()
        .map(InstallOperationSummary::index)
        .chain(
            preview
                .plan()
                .retirements()
                .iter()
                .map(InstallRetirementSummary::index),
        )
        .chain(
            preview
                .plan()
                .hook_activation()
                .map(HookActivationSummary::index),
        )
        .collect::<Vec<_>>();
    assert_eq!(
        stage_indices,
        (0..preview.plan().operation_count()).collect::<Vec<_>>()
    );
    assert!(
        preview
            .plan()
            .operations()
            .iter()
            .all(|stage| stage.kind() == InstallStageKind::FilePublication)
    );
    assert!(
        preview
            .plan()
            .retirements()
            .iter()
            .all(|stage| stage.kind() == InstallStageKind::Retirement)
    );
    assert_eq!(
        preview
            .plan()
            .hook_activation()
            .map(HookActivationSummary::kind),
        Some(InstallStageKind::HookActivation),
    );

    apply_large_migration(&fixture, &migration, &preview, PUBLIC_MAXIMUM);

    let removed = unapply(&fixture.repository).expect("reverse large migration");
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert_public_install_value(&removed, PUBLIC_MAXIMUM);
    assert_eq!(
        fs::read(&fixture.legacy_xtask).expect("read restored large legacy xtask"),
        fixture.xtask_bytes,
    );
    assert_eq!(mode(&fixture.legacy_xtask), EXECUTABLE_MODE);
}

fn apply_large_migration(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
    preview: &InstallPreview,
    public_maximum: usize,
) {
    let digest = migration.digest().expect("large migration has a digest");
    assert_eq!(preview.digest(), &digest);
    rollback::reset_blob_write_count();
    let applied =
        apply(&fixture.repository, Some(&fixture.binary), &digest).expect("apply large migration");
    assert_eq!(
        rollback::blob_write_count(),
        1,
        "a fresh migration writes its large rollback blob exactly once",
    );
    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(applied.completed(), migration.operation_count());
    assert_eq!(applied.plan(), preview.plan());
    assert_public_install_value(&applied, public_maximum);

    rollback::reset_blob_write_count();
    let repeated =
        apply(&fixture.repository, Some(&fixture.binary), &digest).expect("repeat large migration");
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "a complete no-change apply does not write the rollback blob",
    );
    assert_eq!(repeated, applied);
    assert_public_install_value(&repeated, public_maximum);
    assert_private_receipt_manifest(&fixture.repository, public_maximum);
}

#[test]
fn pre_witness_v8_refresh_requires_an_exact_forward_repair_plan() {
    let fixture = version_7_fixture();
    let migration = preview_migration(&fixture);
    let pre_witness_digest = publish_pre_witness_v8(&fixture, &migration);
    let before = installed_state(&fixture.repository, &migration);
    let repair = assert_forward_repair_plan(&fixture, &migration, &pre_witness_digest, &before);
    let other = assert_repair_rejects_another_build(&fixture, &migration, &repair, &before);
    interrupt_and_resume_forward_repair(&fixture, &repair);
    refresh_repaired_appliance(fixture, &other);
}

#[test]
fn complete_v8_appliance_turns_over_only_through_an_exact_v10_refresh() {
    let scratch = Scratch::new("complete-v8-forward-refresh");
    let repository = scratch.repository();
    let binary = scratch.binary(b"versioned appliance");
    let initial = plan(&repository, Some(&binary)).expect("derive initial appliance plan");
    apply(&repository, Some(&binary), &initial.digest().unwrap())
        .expect("install initial appliance");

    let mut historical = read_record(&repository)
        .unwrap()
        .expect("installed receipt exists")
        .record;
    historical.version = FORWARD_MIGRATABLE_RECORD_VERSION;
    historical.plan.version = FORWARD_MIGRATABLE_RECORD_VERSION;
    historical.plan_digest = historical.plan.digest().expect("digest version-8 plan");
    let historical_digest = historical.plan_digest.clone();
    fs::write(
        record_path(&repository).unwrap(),
        wire::encode(&historical).expect("encode complete version-8 receipt"),
    )
    .expect("publish complete version-8 receipt");
    let before = images(&repository, &historical.plan);

    let audit = audit_appliance(&repository, &binary).expect("audit version-8 appliance");
    assert_eq!(audit.state(), ApplianceState::StaleReceipt);
    assert_eq!(audit.receipt(), Some(&historical_digest));
    assert!(audit.detail().contains("version 8"), "{audit:?}");
    assert!(audit.detail().contains("version-10"), "{audit:?}");

    let refresh = plan(&repository, Some(&binary)).expect("derive version-10 forward refresh");
    assert_eq!(refresh.version(), PLAN_VERSION);
    assert_ne!(refresh.digest().unwrap(), historical_digest);
    let refusal = apply(&repository, Some(&binary), &historical_digest)
        .expect_err("the historical digest cannot authorize the forward refresh");
    assert!(matches!(refusal, InstallError::DigestMismatch { .. }));
    assert_eq!(images(&repository, &historical.plan), before);

    let receipt = apply(&repository, Some(&binary), &refresh.digest().unwrap())
        .expect("apply exact version-10 forward refresh");
    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert!(
        audit_appliance(&repository, &binary)
            .expect("audit refreshed appliance")
            .is_current()
    );
}

fn assert_private_receipt_manifest(repository: &Repository, maximum: usize) {
    let bytes = fs::read(record_path(repository).unwrap()).expect("read large private receipt");
    assert!(bytes.len() < maximum);
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).expect("private receipt is JSON");
    assert!(document["plan"]["rollback_archive"]["stored_digest"].is_string());
    assert!(
        document["plan"]["rollback_archive"]
            .get("payload")
            .is_none()
    );
}

#[test]
fn complete_v9_inline_receipt_turns_over_to_one_bounded_v10_blob() {
    const MAX_PRIVATE_RECEIPT: usize = 64 * 1024;
    const MAX_CHECKPOINT_SERIALIZATION: usize = 2 * 1024 * 1024;

    let fixture = version_7_fixture();
    let migration = preview_migration(&fixture);
    let (inline_bytes, inline_digest, blobs) = publish_inline_v9(&fixture, &migration);

    let before = installed_state(&fixture.repository, &migration);
    let preview = assert_v9_preview(&fixture, &migration, &blobs, &before, MAX_PRIVATE_RECEIPT);
    assert_v9_digest_refusal(&fixture, &migration, &blobs, &before, &inline_digest);

    let (published_blob, published_blob_bytes, published_blob_modified) =
        publish_v9_blob_boundary(&fixture, &preview, &blobs, &inline_bytes);

    interrupt_v9_conversion(
        &fixture,
        &preview,
        &published_blob,
        &published_blob_bytes,
        published_blob_modified,
    );
    let current = resume_v9_conversion(
        &fixture,
        &preview,
        &blobs,
        MAX_PRIVATE_RECEIPT,
        MAX_CHECKPOINT_SERIALIZATION,
    );

    assert_v10_no_change(&fixture, &preview, &blobs, &current);

    rollback::reset_deep_decode_count();
    reverse_and_assert_version_7(&fixture);
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "unapply expands and validates its rollback body exactly once",
    );
}

fn publish_inline_v9(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
) -> (Vec<u8>, Digest, PathBuf) {
    apply(
        &fixture.repository,
        Some(&fixture.binary),
        &migration.digest().unwrap(),
    )
    .expect("apply current fixture before publishing version 9");
    let mut inline = read_record(&fixture.repository)
        .unwrap()
        .expect("current receipt exists")
        .record;
    inline.plan.rollback_archive = inline
        .plan
        .rollback_archive
        .as_ref()
        .map(|archive| archive.to_inline(&fixture.repository))
        .transpose()
        .expect("derive exact inline version-9 archive");
    inline.version = INLINE_ROLLBACK_RECORD_VERSION;
    inline.plan.version = INLINE_ROLLBACK_RECORD_VERSION;
    inline.plan_digest = inline.plan.digest().expect("digest inline version-9 plan");
    let inline_digest = inline.plan_digest.clone();
    let inline_bytes = wire::encode(&inline).expect("encode canonical version-9 receipt");
    let mut interrupted = inline.clone();
    interrupted.status = InstallStatus::Applying;
    fs::write(
        record_path(&fixture.repository).unwrap(),
        wire::encode(&interrupted).expect("encode interrupted version-9 receipt"),
    )
    .expect("publish interrupted version-9 receipt");
    let refusal = plan(&fixture.repository, Some(&fixture.binary))
        .expect_err("interrupted version-9 direction cannot convert");
    assert!(matches!(refusal, InstallError::InvalidRecord(_)));
    fs::write(record_path(&fixture.repository).unwrap(), &inline_bytes)
        .expect("publish canonical version-9 receipt");
    let blobs = fixture
        .repository
        .admit_state()
        .unwrap()
        .install_rollback_blobs()
        .to_path_buf();
    fs::remove_dir_all(&blobs).expect("remove current fixture blob before inline conversion");
    (inline_bytes, inline_digest, blobs)
}

fn assert_v9_preview(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
    blobs: &Path,
    before: &InstalledState,
    maximum: usize,
) -> InstallPlan {
    rollback::reset_deep_decode_count();
    let preview = plan(&fixture.repository, Some(&fixture.binary))
        .expect("preview exact version-9 forward conversion");
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "version-9 conversion expands and validates its rollback body exactly once",
    );
    assert_eq!(preview.version(), PLAN_VERSION);
    assert!(preview.canonical_bytes().unwrap().len() < maximum);
    assert!(!blobs.exists(), "preview must remain side-effect free");
    assert_eq!(installed_state(&fixture.repository, migration), *before);
    preview
}

fn assert_v9_digest_refusal(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
    blobs: &Path,
    before: &InstalledState,
    inline_digest: &Digest,
) {
    let refusal = apply(&fixture.repository, Some(&fixture.binary), inline_digest)
        .expect_err("version-9 digest cannot authorize the version-10 receipt");
    assert!(matches!(refusal, InstallError::DigestMismatch { .. }));
    assert!(!blobs.exists());
    assert_eq!(installed_state(&fixture.repository, migration), *before);
}

fn publish_v9_blob_boundary(
    fixture: &MigrationFixture,
    preview: &InstallPlan,
    blobs: &Path,
    inline_bytes: &[u8],
) -> (PathBuf, Vec<u8>, std::time::SystemTime) {
    rollback::reset_blob_write_count();
    {
        let _lock = rollback::acquire_lock(&fixture.repository).unwrap();
        preview
            .publish_staged_rollback(&fixture.repository)
            .expect("publish conversion blob before its receipt CAS");
    }
    assert_eq!(
        rollback::blob_write_count(),
        1,
        "the conversion publishes its large rollback blob with one write",
    );
    assert_eq!(
        fs::read(record_path(&fixture.repository).unwrap()).unwrap(),
        inline_bytes,
        "a publication-boundary interruption leaves the exact version-9 receipt current",
    );
    assert_eq!(fs::read_dir(blobs).unwrap().count(), 1);
    let blob = fs::read_dir(blobs).unwrap().next().unwrap().unwrap().path();
    let bytes = fs::read(&blob).unwrap();
    let modified = fs::metadata(&blob).unwrap().modified().unwrap();
    (blob, bytes, modified)
}

fn interrupt_v9_conversion(
    fixture: &MigrationFixture,
    preview: &InstallPlan,
    blob: &Path,
    blob_bytes: &[u8],
    blob_modified: std::time::SystemTime,
) {
    rollback::reset_deep_decode_count();
    rollback::reset_blob_write_count();
    let interrupted = apply_with_interruption(
        &fixture.repository,
        Some(&fixture.binary),
        &preview.digest().unwrap(),
        Some(0),
    )
    .expect_err("interrupt conversion after its first receipt-bound progress write");
    assert!(matches!(
        interrupted,
        InstallError::Interrupted { operation: 0 }
    ));
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "authorized conversion expands and validates its rollback body exactly once",
    );
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "retrying after blob publication validates and reuses it without another large write",
    );
    let interrupted_record = read_record(&fixture.repository).unwrap().unwrap().record;
    assert_eq!(interrupted_record.version, RECORD_VERSION);
    assert_eq!(interrupted_record.status, InstallStatus::Applying);
    assert_eq!(
        fs::read(blob).unwrap(),
        blob_bytes,
        "same-digest publication reuses the synchronized blob",
    );
    assert_eq!(
        fs::metadata(blob).unwrap().modified().unwrap(),
        blob_modified
    );
}

fn resume_v9_conversion(
    fixture: &MigrationFixture,
    preview: &InstallPlan,
    blobs: &Path,
    receipt_maximum: usize,
    checkpoint_maximum: usize,
) -> Vec<u8> {
    rollback::reset_deep_decode_count();
    rollback::reset_blob_write_count();
    apply(
        &fixture.repository,
        Some(&fixture.binary),
        &preview.digest().unwrap(),
    )
    .expect("resume exact version-10 forward conversion");
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "interrupted conversion resume expands and validates once before mutation",
    );
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "receipt-bound resume reuses the published rollback blob without writing it again",
    );
    let current =
        fs::read(record_path(&fixture.repository).unwrap()).expect("read bounded current receipt");
    assert!(current.len() < receipt_maximum);
    assert!(
        current.len() * (2 * preview.operation_count() + 1) < checkpoint_maximum,
        "checkpoint serialization must remain bounded independently of blob size",
    );
    let document: serde_json::Value =
        serde_json::from_slice(&current).expect("current receipt is JSON");
    assert_eq!(document["version"], PLAN_VERSION);
    assert!(
        document["plan"]["rollback_archive"]
            .get("payload")
            .is_none()
    );
    assert!(document["plan"]["rollback_archive"]["stored_digest"].is_string());
    assert_eq!(fs::read_dir(blobs).unwrap().count(), 1);
    current
}

fn assert_v10_no_change(
    fixture: &MigrationFixture,
    preview: &InstallPlan,
    blobs: &Path,
    current: &[u8],
) {
    let receipt = record_path(&fixture.repository).unwrap();
    let receipt_modified = fs::metadata(&receipt).unwrap().modified().unwrap();
    let blob = fs::read_dir(blobs).unwrap().next().unwrap().unwrap().path();
    let blob_bytes = fs::read(&blob).unwrap();
    let blob_modified = fs::metadata(&blob).unwrap().modified().unwrap();
    rollback::reset_deep_decode_count();
    apply(
        &fixture.repository,
        Some(&fixture.binary),
        &preview.digest().unwrap(),
    )
    .expect("repeat exact current apply without writes");
    assert_eq!(
        rollback::deep_decode_count(),
        0,
        "no-change current admission never expands the rollback body",
    );
    assert_eq!(fs::read(&receipt).unwrap(), current);
    assert_eq!(
        fs::metadata(receipt).unwrap().modified().unwrap(),
        receipt_modified
    );
    assert_eq!(fs::read(&blob).unwrap(), blob_bytes);
    assert_eq!(
        fs::metadata(blob).unwrap().modified().unwrap(),
        blob_modified
    );
}

#[test]
fn relocation_rebinds_one_validated_blob_before_reclaiming_the_old_reference() {
    let mut scratch = Scratch::new("rollback-blob-relocation");
    let repository = scratch.repository();
    let binary = scratch.binary(b"relocatable rollback appliance");
    let initial = plan(&repository, Some(&binary)).expect("plan initial relocation appliance");
    apply(&repository, Some(&binary), &initial.digest().unwrap())
        .expect("install initial relocation appliance");
    record::publish_test_version_7_receipt(&repository)
        .expect("publish relocation version-7 fixture");
    let migration = plan(&repository, Some(&binary)).expect("plan relocation migration");
    apply(&repository, Some(&binary), &migration.digest().unwrap())
        .expect("apply relocation migration");

    let before = read_record(&repository).unwrap().unwrap().record;
    let old_digest = before
        .plan
        .rollback_archive
        .as_ref()
        .and_then(RollbackArchive::stored_digest)
        .cloned()
        .expect("migration receipt references a rollback blob");
    let old_root = scratch.path().to_path_buf();
    let moved_root = old_root.with_extension("relocated");
    fs::rename(&old_root, &moved_root).expect("relocate rollback fixture repository");
    scratch.0 = moved_root;
    let moved = scratch.repository();
    let moved_binary = scratch.join("build/spice");
    let blobs = moved
        .admit_state()
        .unwrap()
        .install_rollback_blobs()
        .to_path_buf();
    let old_blob = blobs.join(format!("{old_digest}.gz"));
    assert!(old_blob.is_file());

    let protected = record::stored_rollback_digest(&moved)
        .expect("read the durable pre-relocation reference")
        .expect("the moved receipt still references its old rollback blob");
    assert_eq!(protected, old_digest);
    {
        let _lock = rollback::acquire_lock(&moved).unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2 * 60 * 60);
        let retained = rollback::reclaim_unreferenced(&moved, Some(&protected), future)
            .expect("retain the blob referenced by the still-durable moved receipt");
        assert_eq!(retained.reclaimable_count, 0);
    }
    assert!(old_blob.is_file());

    rollback::reset_deep_decode_count();
    let relocated = plan(&moved, Some(&moved_binary)).expect("preview relocated appliance");
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "relocation expands and validates the old rollback body exactly once",
    );
    let new_digest = relocated
        .rollback_archive
        .as_ref()
        .and_then(RollbackArchive::stored_digest)
        .cloned()
        .expect("relocated plan references its rebound rollback blob");
    assert_ne!(new_digest, old_digest);
    let new_blob = blobs.join(format!("{new_digest}.gz"));
    assert!(!new_blob.exists(), "relocation preview remains zero-write");
    assert_eq!(fs::read_dir(&blobs).unwrap().count(), 1);

    rollback::reset_deep_decode_count();
    rollback::reset_blob_write_count();
    apply(&moved, Some(&moved_binary), &relocated.digest().unwrap())
        .expect("publish relocated appliance authority");
    assert_eq!(
        rollback::deep_decode_count(),
        1,
        "relocation apply expands and validates the old rollback body exactly once",
    );
    assert_eq!(
        rollback::blob_write_count(),
        1,
        "relocation writes its rebound rollback blob exactly once",
    );
    assert!(old_blob.is_file());
    assert!(new_blob.is_file());
    let current = read_record(&moved).unwrap().unwrap().record;
    assert_eq!(
        current
            .plan
            .rollback_archive
            .as_ref()
            .and_then(RollbackArchive::stored_digest),
        Some(&new_digest),
    );

    reclaim_relocated_blob(&moved, &old_blob, &new_blob, &new_digest);
}

fn reclaim_relocated_blob(
    repository: &Repository,
    old_blob: &Path,
    new_blob: &Path,
    new_digest: &Digest,
) {
    let _lock = rollback::acquire_lock(repository).unwrap();
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2 * 60 * 60);
    let reclaimed = rollback::reclaim_unreferenced(repository, Some(new_digest), future)
        .expect("reclaim superseded relocation blob");
    assert_eq!(reclaimed.reclaimable_count, 1);
    assert!(!old_blob.exists());
    assert!(new_blob.is_file());
}

/// Publish the historical receipt shape that exercises forward-only repair.
fn publish_pre_witness_v8(fixture: &MigrationFixture, migration: &InstallPlan) -> Digest {
    let migrated = apply(
        &fixture.repository,
        Some(&fixture.binary),
        &migration.digest().unwrap(),
    )
    .expect("apply migration before publishing the pre-witness refresh");
    assert_eq!(migrated.status(), InstallStatus::Complete);

    let mut record = read_record(&fixture.repository)
        .unwrap()
        .expect("migrated receipt exists")
        .record;
    record.plan.rollback_archive = record
        .plan
        .rollback_archive
        .as_ref()
        .map(|archive| archive.to_inline(&fixture.repository))
        .transpose()
        .expect("convert fixture rollback archive to exact version-8 shape");
    record.version = FORWARD_MIGRATABLE_RECORD_VERSION;
    record.plan.version = FORWARD_MIGRATABLE_RECORD_VERSION;
    for retirement in &mut record.plan.retirements {
        retirement.superseded = None;
        retirement.rollback = None;
    }
    record.plan_digest = record
        .plan
        .digest()
        .expect("pre-witness refresh has a coherent digest");
    let pre_witness_digest = record.plan_digest.clone();
    fs::write(
        record_path(&fixture.repository).unwrap(),
        wire::encode(&record).expect("encode pre-witness v8 receipt"),
    )
    .expect("publish pre-witness v8 receipt");
    let strict = inspect(&fixture.repository).expect_err("strict inspection must still refuse");
    assert_retirement_witness_refusal(&strict);
    let reversal = unapply(&fixture.repository).expect_err("strict unapply must still refuse");
    assert_retirement_witness_refusal(&reversal);
    pre_witness_digest
}

fn assert_forward_repair_plan(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
    pre_witness_digest: &Digest,
    before: &InstalledState,
) -> InstallPlan {
    let audit = audit_appliance(&fixture.repository, &fixture.binary)
        .expect("audit classifies a bounded repairable receipt");
    assert_eq!(audit.state(), ApplianceState::RepairableReceipt);
    assert_eq!(audit.receipt(), Some(pre_witness_digest));
    assert!(
        audit.detail().contains("can be forward repaired"),
        "{audit:?}"
    );
    assert_eq!(installed_state(&fixture.repository, migration), *before);
    let repair = plan(&fixture.repository, Some(&fixture.binary))
        .expect("derive exact side-effect-free repair plan");
    assert_ne!(repair.digest().unwrap(), *pre_witness_digest);
    assert!(repair.canonical_bytes().unwrap().len() < MAX_COMPACT_PLAN_BYTES);
    let rollback = repair
        .rollback_state(&fixture.repository)
        .expect("decode repaired rollback archive")
        .expect("repair retains rollback state");
    assert!(
        rollback
            .retirements
            .iter()
            .zip(repair.retirements())
            .all(|(image, retirement)| rollback_image_matches(
                image.as_ref(),
                retirement.rollback_witness()
            ))
    );
    assert_eq!(installed_state(&fixture.repository, migration), *before);
    repair
}

fn assert_repair_rejects_another_build(
    fixture: &MigrationFixture,
    migration: &InstallPlan,
    repair: &InstallPlan,
    before: &InstalledState,
) -> PathBuf {
    let other = fixture
        .scratch
        .executable("build/other-spice", b"another current build");
    let stale = apply(&fixture.repository, Some(&other), &repair.digest().unwrap())
        .expect_err("a repair digest cannot authorize another build");
    assert!(matches!(stale, InstallError::DigestMismatch { .. }));
    assert_eq!(installed_state(&fixture.repository, migration), *before);
    other
}

fn interrupt_and_resume_forward_repair(fixture: &MigrationFixture, repair: &InstallPlan) {
    rollback::reset_blob_write_count();
    let interrupted = apply_with_interruption(
        &fixture.repository,
        Some(&fixture.binary),
        &repair.digest().unwrap(),
        Some(0),
    )
    .expect_err("interrupt the exact repair after its first publication");
    assert!(matches!(
        interrupted,
        InstallError::Interrupted { operation: 0 }
    ));
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "the forward repair reuses its already-present content-addressed rollback blob",
    );
    assert_eq!(
        inspect(&fixture.repository)
            .expect("inspect interrupted repair")
            .expect("interrupted repair receipt exists")
            .status(),
        InstallStatus::Applying
    );
    rollback::reset_blob_write_count();
    let repaired = apply(
        &fixture.repository,
        Some(&fixture.binary),
        &repair.digest().unwrap(),
    )
    .expect("resume the exact repair plan");
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "the interrupted repair resume reuses its rollback blob",
    );
    assert_eq!(repaired.status(), InstallStatus::Complete);
    assert!(
        audit_appliance(&fixture.repository, &fixture.binary)
            .expect("audit repaired appliance")
            .is_current()
    );
    assert_eq!(
        inspect(&fixture.repository)
            .expect("inspect repaired receipt")
            .expect("repaired receipt exists")
            .status(),
        InstallStatus::Complete
    );
}

fn refresh_repaired_appliance(fixture: MigrationFixture, other: &Path) {
    let refresh = plan(&fixture.repository, Some(other))
        .expect("derive a normal refresh after the forward repair");
    let rollback = refresh
        .rollback_state(&fixture.repository)
        .expect("decode refreshed rollback archive")
        .expect("refresh retains rollback state");
    assert!(
        rollback
            .retirements
            .iter()
            .zip(refresh.retirements())
            .all(|(image, retirement)| {
                retirement.superseded.is_none()
                    && rollback_image_matches(image.as_ref(), retirement.rollback_witness())
            })
    );
    assert!(refresh.canonical_bytes().unwrap().len() < MAX_COMPACT_PLAN_BYTES);
    rollback::reset_blob_write_count();
    apply(&fixture.repository, Some(other), &refresh.digest().unwrap())
        .expect("apply a normal refresh after the forward repair");
    assert_eq!(
        rollback::blob_write_count(),
        0,
        "ordinary refresh checkpoints reuse the digest-bound rollback manifest",
    );
    assert!(
        audit_appliance(&fixture.repository, other)
            .expect("audit refreshed repaired appliance")
            .is_current()
    );
    reverse_and_assert_version_7(&fixture);
}

#[derive(Clone, Copy, Debug)]
enum RetirementTamper {
    Bytes,
    Mode,
    Removed,
    Invented,
    Witness,
}

#[derive(Debug, Eq, PartialEq)]
struct InstalledState {
    receipt: Vec<u8>,
    operations: Vec<Option<ObservedFile>>,
    retirements: Vec<Option<ObservedFile>>,
    hooks: Vec<String>,
}

#[test]
fn resealed_retirement_images_must_match_their_exact_plan_witnesses() {
    for tamper in [
        RetirementTamper::Bytes,
        RetirementTamper::Mode,
        RetirementTamper::Removed,
        RetirementTamper::Invented,
        RetirementTamper::Witness,
    ] {
        retirement_tamper_refuses_inertly(tamper);
    }
}

fn retirement_tamper_refuses_inertly(tamper: RetirementTamper) {
    let fixture = compact_version_7_fixture();
    if matches!(tamper, RetirementTamper::Invented) {
        fs::remove_file(&fixture.legacy_guard).expect("remove absent-image fixture sidecar");
    }
    let migration =
        plan(&fixture.repository, Some(&fixture.binary)).expect("derive migration to tamper");
    let migrated = apply(
        &fixture.repository,
        Some(&fixture.binary),
        &migration.digest().unwrap(),
    )
    .expect("apply migration before receipt tampering");
    assert_eq!(migrated.status(), InstallStatus::Complete);

    let mut record = read_record(&fixture.repository)
        .unwrap()
        .expect("migrated receipt exists")
        .record;
    let mut rollback = record
        .plan
        .rollback_state(&fixture.repository)
        .expect("decode rollback before tampering")
        .expect("migration retains rollback state");
    match tamper {
        RetirementTamper::Bytes => {
            let image = rollback.retirements[0]
                .as_ref()
                .expect("xtask rollback image exists");
            let mut bytes = image.bytes().to_vec();
            bytes[0] ^= 0xff;
            rollback.retirements[0] = Some(FileImage::new(image.mode(), bytes));
        }
        RetirementTamper::Mode => {
            let image = rollback.retirements[0]
                .as_ref()
                .expect("xtask rollback image exists");
            rollback.retirements[0] = Some(FileImage::new(0o700, image.bytes().to_vec()));
        }
        RetirementTamper::Removed => rollback.retirements[0] = None,
        RetirementTamper::Invented => {
            assert!(record.plan.retirements[1].superseded.is_none());
            assert!(rollback.retirements[1].is_none());
            rollback.retirements[1] = Some(FileImage::new(DATA_MODE, b"invented\n".to_vec()));
        }
        RetirementTamper::Witness => {
            assert!(record.plan.retirements[0].superseded.is_some());
            record.plan.retirements[0].rollback =
                Some(GeneratedImage::new(DATA_MODE, b"conflicting witness\n"));
        }
    }
    record.plan.rollback_archive =
        Some(RollbackArchive::new(&rollback).expect("reseal coherent tampered rollback archive"));
    record
        .plan
        .publish_staged_rollback(&fixture.repository)
        .expect("publish resealed tampered rollback blob");
    record.plan_digest = record
        .plan
        .digest()
        .expect("resealed plan has a coherent digest");
    fs::write(
        record_path(&fixture.repository).unwrap(),
        wire::encode(&record).expect("encode coherent tampered receipt"),
    )
    .expect("publish coherent tampered receipt");

    let before = installed_state(&fixture.repository, &migration);
    if matches!(tamper, RetirementTamper::Witness) {
        let refusal = plan(&fixture.repository, Some(&fixture.binary))
            .expect_err("receipt-visible witness mismatch refuses shallow admission");
        assert_retirement_witness_refusal(&refusal);
    } else {
        plan(&fixture.repository, Some(&fixture.binary))
            .expect("ordinary complete admission does not expand rollback bodies");
    }
    assert_eq!(installed_state(&fixture.repository, &migration), before);

    let reversal = unapply(&fixture.repository)
        .expect_err("unapply must semantically validate rollback before mutation");
    assert_retirement_witness_refusal(&reversal);
    assert_eq!(installed_state(&fixture.repository, &migration), before);
}

fn assert_retirement_witness_refusal(error: &InstallError) {
    assert!(
        matches!(error, InstallError::InvalidRecord(detail)
            if detail == "migration rollback archive does not match its retirement witnesses"),
        "{error:?}"
    );
}

fn installed_state(repository: &Repository, migration: &InstallPlan) -> InstalledState {
    InstalledState {
        receipt: fs::read(record_path(repository).unwrap()).expect("read receipt state"),
        operations: images(repository, migration),
        retirements: migration
            .retirements
            .iter()
            .map(|retirement| observe(&retirement.destination(repository)).unwrap())
            .collect(),
        hooks: git_values(
            repository.worktree_root(),
            &["config", "--local", "--get-all", "core.hooksPath"],
        ),
    }
}

fn version_7_fixture() -> MigrationFixture {
    version_7_fixture_with_assets(
        vec![b's'; LARGE_BINARY_BYTES],
        vec![b'x'; LARGE_BINARY_BYTES],
    )
}

fn compact_version_7_fixture() -> MigrationFixture {
    version_7_fixture_with_assets(
        b"current spice fixture".to_vec(),
        b"retired xtask fixture".to_vec(),
    )
}

fn version_7_fixture_with_assets(binary_bytes: Vec<u8>, xtask_bytes: Vec<u8>) -> MigrationFixture {
    let scratch = Scratch::new("version-7-migration");
    let repository = scratch.repository();
    let binary = scratch.binary(&binary_bytes);
    let initial = plan(&repository, Some(&binary)).expect("plan fixture appliance");
    apply(&repository, Some(&binary), &initial.digest().unwrap())
        .expect("install fixture appliance");
    let (prior, prior_record) = publish_version_7_receipt(&repository);

    let legacy_xtask = repository.common_dir().join(RETIRED_XTASK_PATH);
    let legacy_guard = repository.common_dir().join(RETIRED_XTASK_GUARD_PATH);
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
        .rollback_state(&fixture.repository)
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

    reverse_and_assert_version_7(&fixture);
    assert_eq!(
        images(&fixture.repository, &fixture.prior.plan),
        fixture.prior_assets
    );
}

fn reverse_and_assert_version_7(fixture: &MigrationFixture) {
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
        images(&fixture.repository, &fixture.prior.plan)
            .into_iter()
            .map(|observed| observed.map(|observed| observed.image))
            .collect::<Vec<_>>(),
        fixture
            .prior_assets
            .iter()
            .map(|observed| observed.as_ref().map(|observed| observed.image.clone()))
            .collect::<Vec<_>>()
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

fn deterministic_bytes(length: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn assert_public_install_value(value: &impl Serialize, maximum: usize) {
    let bytes = wire::encode(value).expect("public install value encodes");
    assert!(
        bytes.len() <= maximum,
        "public install value grew to {} bytes",
        bytes.len(),
    );
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).expect("public install value is JSON");
    assert_no_private_install_key(&value);
}

fn assert_no_private_install_key(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                assert!(
                    !matches!(name.as_str(), "payload" | "bytes" | "repository"),
                    "public install value exposed private field {name:?}",
                );
                assert_no_private_install_key(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                assert_no_private_install_key(value);
            }
        }
        _ => {}
    }
}
