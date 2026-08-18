use super::*;

#[test]
fn unapply_resumes_a_write_interrupted_before_progress_publication() {
    const INTERRUPTION: usize = 2;

    let scratch = Scratch::new("unapply-resume");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();

    let error = unapply_with_interruption(&repository, Some(INTERRUPTION)).unwrap_err();
    assert!(matches!(
        error,
        InstallError::Interrupted {
            operation: INTERRUPTION
        }
    ));
    let interrupted = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(interrupted.status, InstallStatus::Unapplying);
    assert_eq!(
        interrupted.completed,
        planned.operation_count() - 1 - INTERRUPTION
    );
    assert_eq!(
        plan(&repository, None).unwrap().digest().unwrap(),
        planned.digest().unwrap()
    );

    let completed = unapply(&repository).unwrap();
    assert_eq!(completed.status(), InstallStatus::Uninstalled);
    assert!(planned.operations().iter().all(|operation| {
        observe(&operation.destination(&repository))
            .unwrap()
            .map(|observed| observed.image)
            == operation.previous().cloned()
    }));
}

#[test]
fn operator_drift_refuses_unapply_before_any_reversal() {
    let scratch = Scratch::new("drift");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    let generated = images(&repository, &planned);
    fs::write(
        repository.common_dir().join(ENTRY_PATH),
        b"operator-owned replacement\n",
    )
    .unwrap();

    let error = unapply(&repository).unwrap_err();
    assert!(matches!(
        error,
        InstallError::Drift { path, .. } if path == ENTRY_PATH
    ));
    let record = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(record.status, InstallStatus::Complete);
    for (index, operation) in planned.operations().iter().enumerate() {
        if operation.path() != ENTRY_PATH {
            assert_eq!(
                observe(&operation.destination(&repository)).unwrap(),
                generated[index]
            );
        }
    }
}

#[test]
fn malformed_record_refuses_and_foreign_exclusions_are_ignored() {
    let malformed = Scratch::new("malformed");
    let repository = malformed.repository();
    fs::create_dir_all(record_path(&repository).unwrap().parent().unwrap()).unwrap();
    fs::write(
        record_path(&repository).unwrap(),
        b"{\"unexpected\":true}\n",
    )
    .unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::Decode(_)));
    assert!(!repository.common_dir().join(ENTRY_PATH).exists());

    let foreign = Scratch::new("foreign-exclusions");
    let repository = foreign.repository();
    let bytes = b"# BEGIN Spice managed assets\nwrong\n# BEGIN Spice managed assets\n\xff";
    fs::write(foreign.exclude(), bytes).unwrap();
    let planned = plan(&repository, None).expect("foreign exclusions do not affect planning");
    assert!(
        planned
            .operations()
            .iter()
            .all(|operation| operation.asset() != Asset::Exclusions)
    );
    assert_eq!(fs::read(foreign.exclude()).unwrap(), bytes);
    assert!(!record_path(&repository).unwrap().exists());
}

#[test]
fn python_state_root_is_not_an_install_input() {
    let scratch = Scratch::new("python-state");
    let repository = scratch.repository();
    let python = repository.common_dir().join(".spice");
    let sentinels = [
        python.join("install"),
        python.join("install.json"),
        python.join("rust/install.json"),
    ];
    for (index, path) in sentinels.iter().enumerate() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("python-owned-{index}\n")).unwrap();
    }
    let before = sentinels
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();

    let preview = plan(&repository, None).expect("Python state cannot block Rust preview");

    assert!(!preview.operations().is_empty());
    assert_eq!(
        sentinels
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(ENTRY_PATH).exists());
}

#[test]
fn prior_current_path_record_names_its_owning_release_before_mutation() {
    for (version, owner) in LEGACY_RECORD_VERSIONS {
        let scratch = Scratch::new(owner);
        let repository = scratch.repository();
        let planned = plan(&repository, None).unwrap();
        apply(&repository, None, &planned.digest().unwrap()).unwrap();
        let installed = images(&repository, &planned);
        let mut record = read_record(&repository).unwrap().unwrap().record;
        record.version = version;
        if version == 5 {
            record.plan.version = version;
            record.plan.operations[0].asset = Asset::Exclusions;
        }
        let mut bytes = wire::encode(&record).unwrap();
        if version == 4 {
            let mut document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let worktree = serde_json::to_value(repository.worktree_root()).unwrap();
            document["repository"]["worktree"] = worktree.clone();
            document["plan"]["repository"]["worktree"] = worktree;
            bytes = serde_json::to_vec(&document).unwrap();
        }
        fs::write(record_path(&repository).unwrap(), bytes).unwrap();

        let error = plan(&repository, None).unwrap_err();
        assert!(matches!(
            &error,
            InstallError::LegacyRecord { path, owner: observed }
                if path == &record_path(&repository).unwrap() && observed == &owner
        ));
        assert!(error.to_string().contains(owner));
        assert_eq!(images(&repository, &planned), installed);
    }
}

#[test]
fn record_digest_tampering_refuses_before_unapply_mutation() {
    let scratch = Scratch::new("record-digest");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    let generated = images(&repository, &planned);
    let mut record = read_record(&repository).unwrap().unwrap().record;
    record.plan_digest = Digest::compute(Domain::InstallPlan, b"tampered");
    fs::write(
        record_path(&repository).unwrap(),
        wire::encode(&record).unwrap(),
    )
    .unwrap();

    let error = unapply(&repository).unwrap_err();
    assert!(matches!(error, InstallError::InvalidRecord(_)));
    assert_eq!(images(&repository, &planned), generated);
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum RollbackBlobTamper {
    Missing,
    Content,
    Truncated,
    Symlink,
    HardLink,
    Mode,
    ManifestLength,
    ManifestLimit,
}

#[cfg(unix)]
#[test]
fn rollback_blob_shape_content_and_manifest_fail_closed_before_install_mutation() {
    for (index, tamper) in [
        RollbackBlobTamper::Missing,
        RollbackBlobTamper::Content,
        RollbackBlobTamper::Truncated,
        RollbackBlobTamper::Symlink,
        RollbackBlobTamper::HardLink,
        RollbackBlobTamper::Mode,
        RollbackBlobTamper::ManifestLength,
        RollbackBlobTamper::ManifestLimit,
    ]
    .into_iter()
    .enumerate()
    {
        rollback_blob_refusal_case(index, tamper);
    }
}

#[cfg(unix)]
fn rollback_blob_refusal_case(index: usize, tamper: RollbackBlobTamper) {
    let (scratch, repository, binary, migration, blobs, blob) = rollback_blob_fixture(index);
    let before_receipt = fs::read(record_path(&repository).unwrap()).unwrap();
    let before_images = images(&repository, &migration);
    tamper_rollback_blob(tamper, &scratch, &repository, &blobs, &blob);
    let refusal = plan(&repository, Some(&binary)).expect_err("unsafe rollback blob must refuse");
    assert!(
        matches!(
            refusal,
            InstallError::InvalidRecord(_) | InstallError::Io { .. } | InstallError::State(_)
        ),
        "unexpected rollback refusal: {refusal:?}",
    );
    assert_eq!(images(&repository, &migration), before_images);
    if !matches!(
        tamper,
        RollbackBlobTamper::ManifestLength | RollbackBlobTamper::ManifestLimit
    ) {
        assert_eq!(
            fs::read(record_path(&repository).unwrap()).unwrap(),
            before_receipt,
        );
    }
}

#[cfg(unix)]
fn rollback_blob_fixture(
    index: usize,
) -> (Scratch, Repository, PathBuf, InstallPlan, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new(&format!("rollback-blob-refusal-{index}"));
    let repository = scratch.repository();
    let binary = scratch.binary(b"rollback blob fixture executable");
    let initial = plan(&repository, Some(&binary)).expect("plan initial fixture appliance");
    apply(&repository, Some(&binary), &initial.digest().unwrap())
        .expect("install initial fixture appliance");
    record::publish_test_version_7_receipt(&repository).expect("publish version-7 fixture receipt");
    let migration = plan(&repository, Some(&binary)).expect("plan rollback-blob fixture migration");
    apply(&repository, Some(&binary), &migration.digest().unwrap())
        .expect("apply rollback-blob fixture migration");
    let blobs = repository
        .admit_state()
        .unwrap()
        .install_rollback_blobs()
        .to_path_buf();
    assert_eq!(
        fs::metadata(&blobs).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    let blob = fs::read_dir(&blobs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "gz"))
        .expect("fixture owns one rollback blob");
    (scratch, repository, binary, migration, blobs, blob)
}

#[cfg(unix)]
fn tamper_rollback_blob(
    tamper: RollbackBlobTamper,
    scratch: &Scratch,
    repository: &Repository,
    blobs: &Path,
    blob: &Path,
) {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    match tamper {
        RollbackBlobTamper::Missing => fs::remove_file(blob).unwrap(),
        RollbackBlobTamper::Content => {
            let mut bytes = fs::read(blob).unwrap();
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xff;
            fs::write(blob, bytes).unwrap();
        }
        RollbackBlobTamper::Truncated => {
            let file = fs::OpenOptions::new().write(true).open(blob).unwrap();
            file.set_len(file.metadata().unwrap().len() - 1).unwrap();
        }
        RollbackBlobTamper::Symlink => {
            let outside = scratch.join("outside-rollback");
            fs::write(&outside, fs::read(blob).unwrap()).unwrap();
            fs::remove_file(blob).unwrap();
            symlink(outside, blob).unwrap();
        }
        RollbackBlobTamper::HardLink => {
            fs::hard_link(blob, blobs.join("unreferenced-peer")).unwrap();
        }
        RollbackBlobTamper::Mode => {
            fs::set_permissions(blob, fs::Permissions::from_mode(0o644)).unwrap();
        }
        RollbackBlobTamper::ManifestLength | RollbackBlobTamper::ManifestLimit => {
            tamper_rollback_manifest(tamper, repository, blob);
        }
    }
}

#[cfg(unix)]
fn tamper_rollback_manifest(tamper: RollbackBlobTamper, repository: &Repository, blob: &Path) {
    let mut record = read_record(repository).unwrap().unwrap().record;
    let stored_bytes = if matches!(tamper, RollbackBlobTamper::ManifestLimit) {
        u64::MAX
    } else {
        fs::metadata(blob).unwrap().len() + 1
    };
    record
        .plan
        .rollback_archive
        .as_mut()
        .unwrap()
        .set_stored_bytes_for_test(stored_bytes);
    record.plan_digest = record.plan.digest().unwrap();
    fs::write(
        record_path(repository).unwrap(),
        wire::encode(&record).unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn rollback_blob_reclamation_protects_current_and_retains_unsafe_candidates() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, SystemTime};

    let scratch = Scratch::new("rollback-blob-reclamation");
    let repository = scratch.repository();
    let binary = scratch.binary(b"rollback cleanup fixture executable");
    let initial = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &initial.digest().unwrap()).unwrap();
    record::publish_test_version_7_receipt(&repository).unwrap();
    let migration = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &migration.digest().unwrap()).unwrap();

    let record = read_record(&repository).unwrap().unwrap().record;
    let protected = record
        .plan
        .rollback_archive
        .as_ref()
        .and_then(RollbackArchive::stored_digest)
        .cloned()
        .expect("current receipt names one protected blob");
    let directory = repository
        .admit_state()
        .unwrap()
        .install_rollback_blobs()
        .to_path_buf();
    let current = directory.join(format!("{protected}.gz"));
    let current_bytes = fs::read(&current).unwrap();
    let orphan = directory.join(format!("{}.gz", "a".repeat(64)));
    fs::write(&orphan, b"unreferenced").unwrap();
    fs::set_permissions(&orphan, fs::Permissions::from_mode(0o600)).unwrap();
    let staged = directory.join(format!(
        ".spice-install-rollback-{}-999.tmp",
        std::process::id()
    ));
    fs::write(&staged, b"staged").unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
    let unsafe_name = directory.join(format!("{}.gz", "b".repeat(64)));
    fs::write(&unsafe_name, b"unsafe").unwrap();
    fs::set_permissions(&unsafe_name, fs::Permissions::from_mode(0o644)).unwrap();

    let _lock = rollback::acquire_lock(&repository).unwrap();
    let future = SystemTime::now() + Duration::from_secs(2 * 60 * 60);
    let inspection = rollback::inspect_unreferenced(&repository, Some(&protected), future).unwrap();
    assert_eq!(inspection.reclaimable_count, 2);
    assert!(orphan.exists());
    assert!(staged.exists());
    assert_eq!(fs::read(&current).unwrap(), current_bytes);
    assert!(unsafe_name.exists());
    assert!(
        inspection
            .unsafe_candidates
            .iter()
            .any(|candidate| candidate.contains(&"b".repeat(64)))
    );
    let report = rollback::reclaim_unreferenced(&repository, Some(&protected), future).unwrap();
    assert_eq!(report.reclaimable_count, 2);
    assert!(!orphan.exists());
    assert!(!staged.exists());
    assert_eq!(fs::read(current).unwrap(), current_bytes);
    assert!(unsafe_name.exists());
    assert!(
        report
            .unsafe_candidates
            .iter()
            .any(|candidate| candidate.contains(&"b".repeat(64)))
    );
}

#[test]
fn complete_admission_is_shallow_but_unapply_validates_the_rollback_body() {
    let scratch = Scratch::new("rollback-blob-semantic-boundary");
    let repository = scratch.repository();
    let binary = scratch.binary(b"rollback semantic fixture executable");
    let initial = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &initial.digest().unwrap()).unwrap();
    record::publish_test_version_7_receipt(&repository).unwrap();
    let migration = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &migration.digest().unwrap()).unwrap();

    let directory = repository
        .admit_state()
        .unwrap()
        .install_rollback_blobs()
        .to_path_buf();
    let blob = fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let invalid_compressed = vec![0xa5; fs::metadata(&blob).unwrap().len() as usize];
    let mut record = read_record(&repository).unwrap().unwrap().record;
    record
        .plan
        .rollback_archive
        .as_mut()
        .unwrap()
        .reseal_stored_bytes_for_test(&invalid_compressed);
    let replacement_blob = directory.join(format!(
        "{}.gz",
        record
            .plan
            .rollback_archive
            .as_ref()
            .and_then(RollbackArchive::stored_digest)
            .unwrap()
    ));
    record.plan_digest = record.plan.digest().unwrap();
    fs::rename(&blob, &replacement_blob).unwrap();
    fs::write(&replacement_blob, &invalid_compressed).unwrap();
    fs::write(
        record_path(&repository).unwrap(),
        wire::encode(&record).unwrap(),
    )
    .unwrap();
    let before_receipt = fs::read(record_path(&repository).unwrap()).unwrap();
    let before_images = images(&repository, &migration);

    let preview = plan(&repository, Some(&binary))
        .expect("complete-state admission verifies stored bytes without expanding them");
    assert_eq!(preview.digest().unwrap(), record.plan_digest);
    assert_eq!(images(&repository, &migration), before_images);

    let refusal = unapply(&repository).expect_err("unapply must expand before mutation");
    assert!(
        matches!(refusal, InstallError::InvalidRecord(reason) if reason.contains("decompress"))
    );
    assert_eq!(images(&repository, &migration), before_images);
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        before_receipt,
    );
}

#[cfg(unix)]
#[test]
fn planning_binds_internal_symlinked_install_parents_and_refuses_escape() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("symlink");
    let repository = scratch.repository();
    let outside = repository.common_dir().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, repository.common_dir().join(".spice-install")).unwrap();

    let planned = plan(&repository, None).unwrap();
    let skill = planned
        .operations()
        .iter()
        .find(|operation| operation.asset() == Asset::Skill)
        .unwrap();
    assert_eq!(skill.destination, "outside/agents/skills/spice/SKILL.md");
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(outside.join("agents/skills/spice/SKILL.md").is_file());
    unapply(&repository).unwrap();

    let escaped = Scratch::new("symlink-escape");
    let repository = escaped.repository();
    let external = escaped.0.with_extension("external");
    fs::create_dir(&external).unwrap();
    let canonical_external = fs::canonicalize(&external).unwrap();
    symlink(&external, repository.common_dir().join(".spice-install")).unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        error,
        InstallError::PathOutsideRoot(path) if path == canonical_external
    ));
    assert!(
        plan(&repository, None)
            .unwrap_err()
            .to_string()
            .contains("canonical permitted root")
    );
    assert!(fs::read_dir(&external).unwrap().next().is_none());
    fs::remove_dir(external).unwrap();

    let state = Scratch::new("state-symlink");
    let repository = state.repository();
    let outside = state.join("outside-state");
    fs::create_dir(&outside).unwrap();
    let state_root = repository
        .common_dir()
        .join(crate::repository::STATE_DIRECTORY);
    symlink(&outside, &state_root).unwrap();

    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        error,
        InstallError::State(crate::repository::StateError::Linked { path })
            if path == state_root
    ));
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}
