use super::*;

/// An intact completed appliance is the authority for its own old fixed bytes.
///
/// The installed binary makes this a build-bearing receipt. A linked worktree
/// can therefore preview the current build over it without first withdrawing
/// anything, while reversal through the new build remains refused: upgrade is
/// the one current path.
#[test]
fn a_linked_worktree_refreshes_all_intact_prior_build_assets_without_unapplying() {
    let scratch = Scratch::new("refresh-prior-fixed");
    let repository = scratch.repository();
    let binary = scratch.binary(b"one installed build\n");
    let linked_repository = linked_repository(&scratch, &repository);
    let installed = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &installed.digest().unwrap()).unwrap();
    let historical = install_historical_assets(&repository);
    fs::write(&binary, b"current selected build\n").unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);
    let prior_record = fs::read(record_path(&repository).unwrap()).unwrap();
    let prior_images = images(&repository, &installed);

    let reversal = unapply(&linked_repository)
        .expect_err("another build must refresh rather than reverse the live receipt");
    assert!(
        matches!(reversal, InstallError::InvalidRecord(_)),
        "cross-build reversal refused for the wrong reason: {reversal}"
    );
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        prior_record
    );
    assert_eq!(images(&repository, &installed), prior_images);
    let refreshed = plan(&linked_repository, Some(&binary))
        .expect("an intact prior build derives one current refresh");
    for (asset, generated) in &historical {
        let operation = refreshed
            .operations()
            .iter()
            .find(|operation| operation.asset() == *asset)
            .expect("the refresh owns every fixed generated asset");
        assert_eq!(operation.superseded(), Some(generated));
    }
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        prior_record
    );
    assert_eq!(images(&repository, &installed), prior_images);

    let digest = refreshed.digest().unwrap();
    apply(&linked_repository, Some(&binary), &digest)
        .expect("the exact cross-build refresh applies");
    for (asset, _) in historical {
        let operation = refreshed
            .operations()
            .iter()
            .find(|operation| operation.asset() == asset)
            .expect("the current plan keeps every fixed generated asset");
        let expected =
            materialize_generated(operation, Some(&binary), repository.common_dir()).unwrap();
        assert_eq!(
            fs::read(operation.destination(&repository)).unwrap(),
            expected.bytes()
        );
    }
    assert_eq!(
        fs::read(installed_binary(&repository)).unwrap(),
        b"current selected build\n"
    );
    assert_eq!(
        unapply(&linked_repository)
            .expect("reverse the cross-build refresh")
            .status(),
        InstallStatus::Uninstalled
    );
    assert!(
        PUBLISHED_PATHS
            .into_iter()
            .all(|path| !repository.common_dir().join(path).exists())
    );
}

fn install_historical_assets(repository: &Repository) -> [(Asset, GeneratedImage); 7] {
    [
        (Asset::GitPreCommit, EXECUTABLE_MODE),
        (Asset::GitCommitMessage, EXECUTABLE_MODE),
        (Asset::GitReferenceTransaction, EXECUTABLE_MODE),
        (Asset::HookEntry, EXECUTABLE_MODE),
        (Asset::HookStatic, EXECUTABLE_MODE),
        (Asset::HookWrapperSpice, EXECUTABLE_MODE),
        (Asset::Skill, DATA_MODE),
    ]
    .map(|(asset, mode)| {
        let bytes = format!("prior build generated {asset:?}\n");
        let generated =
            install_completed_historical_asset(repository, asset, mode, bytes.as_bytes());
        (asset, generated)
    })
}

fn linked_repository(scratch: &Scratch, repository: &Repository) -> Repository {
    let linked = scratch.join("linked");
    let added = std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ])
        .current_dir(repository.worktree_root())
        .status()
        .expect("launch linked-worktree fixture creation");
    assert!(added.success());
    Repository::discover(&linked).unwrap()
}

/// A rebuilt executable is explicit authority to replace one exact drifted
/// executable, but it does not rebase the install's eventual reversal image.
#[test]
fn a_rebuilt_binary_reconciles_exact_binary_drift_and_keeps_the_first_reversal() {
    let scratch = Scratch::new("refresh-binary-drift");
    let repository = scratch.repository();
    let binary = scratch.binary(b"first build\n");
    let installed_path = repository.common_dir().join(BINARY_PATH);
    let original_exclusions = fs::read(scratch.exclude()).unwrap();

    let first = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &first.digest().unwrap()).unwrap();
    fs::write(&installed_path, b"unrecorded shared build\n").unwrap();
    set_test_mode(&installed_path, 0o700);
    let drifted = observe(&installed_path)
        .unwrap()
        .expect("the drifted executable is regular")
        .generated();
    fs::write(&binary, b"current source build\n").unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);

    let refreshed = plan(&repository, Some(&binary))
        .expect("binary-only drift produces an exact reconciliation preview");
    let repeated = plan(&repository, Some(&binary)).expect("the preview is canonical");
    assert_eq!(refreshed, repeated);
    let operation = refreshed
        .operations()
        .iter()
        .find(|operation| operation.asset() == Asset::Binary)
        .expect("the refresh owns the executable");
    assert_eq!(operation.superseded(), Some(&drifted));
    assert_eq!(
        operation.previous(),
        None,
        "reconciliation rebased unapply onto the unrecorded build"
    );

    let digest = refreshed.digest().unwrap();
    let receipt = apply(&repository, Some(&binary), &digest)
        .expect("the exact reconciliation preview applies");
    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(
        fs::read(&installed_path).unwrap(),
        b"current source build\n"
    );
    assert_eq!(
        plan(&repository, Some(&binary)).unwrap().digest().unwrap(),
        digest,
        "the reconciled build is not idempotent"
    );
    assert_eq!(
        apply(&repository, Some(&binary), &digest).unwrap(),
        receipt,
        "repeating the reconciled apply changed its receipt"
    );

    unapply(&repository).expect("the reconciled install reverses");
    assert!(!installed_path.exists());
    assert_eq!(fs::read(scratch.exclude()).unwrap(), original_exclusions);
}

/// Reconciliation is confined to the executable. One second drifted asset
/// keeps the complete receipt authoritative and refuses before any write.
#[test]
fn a_rebuilt_binary_does_not_reconcile_non_binary_drift() {
    let scratch = Scratch::new("refresh-other-drift");
    let repository = scratch.repository();
    let binary = scratch.binary(b"first build\n");
    let first = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &first.digest().unwrap()).unwrap();

    fs::write(
        repository.common_dir().join(ENTRY_PATH),
        b"operator hook replacement\n",
    )
    .unwrap();
    fs::write(
        repository.common_dir().join(BINARY_PATH),
        b"unrecorded shared build\n",
    )
    .unwrap();
    fs::write(&binary, b"current source build\n").unwrap();
    let before_record = fs::read(record_path(&repository).unwrap()).unwrap();
    let before = images(&repository, &first);

    let error =
        plan(&repository, Some(&binary)).expect_err("non-binary drift remains a strict refusal");
    assert!(matches!(error, InstallError::Drift { path, .. } if path == ENTRY_PATH));
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        before_record
    );
    assert_eq!(images(&repository, &first), before);
}

/// The observed drift image is part of the preview identity. Moving it before
/// apply derives a different plan and refuses the stale digest without writes.
#[test]
fn a_binary_drift_race_refuses_the_preview_before_mutation() {
    let scratch = Scratch::new("refresh-binary-race");
    let repository = scratch.repository();
    let binary = scratch.binary(b"first build\n");
    let first = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &first.digest().unwrap()).unwrap();

    let installed_path = repository.common_dir().join(BINARY_PATH);
    fs::write(&installed_path, b"previewed drift\n").unwrap();
    fs::write(&binary, b"current source build\n").unwrap();
    let stale = plan(&repository, Some(&binary))
        .expect("the first exact drift image previews")
        .digest()
        .unwrap();
    fs::write(&installed_path, b"competing drift\n").unwrap();
    let before_record = fs::read(record_path(&repository).unwrap()).unwrap();
    let before = images(&repository, &first);

    let error = apply(&repository, Some(&binary), &stale)
        .expect_err("moving the observed drift invalidates its preview");
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(fs::read(&installed_path).unwrap(), b"competing drift\n");
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        before_record
    );
    assert_eq!(images(&repository, &first), before);
}

/// A refresh is a rebuilt executable, never a differently shaped install.
///
/// Deriving one from a plan that drops the binary would publish a plan with no
/// operation owning the installed executable, so reversing it later would leave
/// that file behind. Both directions of the disagreement stay refusals, because
/// neither says anything about which build the destination holds.
#[test]
fn a_complete_install_refuses_a_plan_that_adds_or_drops_the_executable() {
    let dropped = Scratch::new("refresh-drops-binary");
    let repository = dropped.repository();
    let binary = dropped.binary(b"old binary bytes");
    let installed = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    apply(&repository, Some(&binary), &installed).unwrap();

    let error = plan(&repository, None).expect_err("a plan without the installed executable");
    assert!(
        matches!(error, InstallError::BinaryContextChanged),
        "{error:?}"
    );
    assert!(repository.common_dir().join(BINARY_PATH).is_file());

    let added = Scratch::new("refresh-adds-binary");
    let repository = added.repository();
    let installed = plan(&repository, None).unwrap().digest().unwrap();
    apply(&repository, None, &installed).unwrap();
    let binary = added.binary(b"new binary bytes");

    let error =
        plan(&repository, Some(&binary)).expect_err("a plan adding an executable to an install");
    assert!(
        matches!(error, InstallError::BinaryContextChanged),
        "{error:?}"
    );
    assert!(!repository.common_dir().join(BINARY_PATH).exists());
}

/// Apply is a second door onto the refresh branch, and it refuses the same way.
///
/// Apply derives its own plan rather than reusing `plan`, so a shape
/// disagreement reaching its refresh branch would be caught only incidentally,
/// by the derived plan failing to match the digest the caller named. That
/// reports `DigestMismatch` — a stale-plan answer — for a caller that is not
/// stale at all, so both directions have to refuse at the binary context here
/// too, before the digest is ever compared.
#[test]
fn a_complete_install_refuses_an_apply_that_adds_or_drops_the_executable() {
    let dropped = Scratch::new("apply-drops-binary");
    let repository = dropped.repository();
    let binary = dropped.binary(b"old binary bytes");
    let installed = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    apply(&repository, Some(&binary), &installed).unwrap();

    let error = apply(&repository, None, &installed)
        .expect_err("an apply without the installed executable");
    assert!(
        matches!(error, InstallError::BinaryContextChanged),
        "{error:?}"
    );
    assert!(repository.common_dir().join(BINARY_PATH).is_file());

    let added = Scratch::new("apply-adds-binary");
    let repository = added.repository();
    let installed = plan(&repository, None).unwrap().digest().unwrap();
    apply(&repository, None, &installed).unwrap();
    let binary = added.binary(b"new binary bytes");

    let error = apply(&repository, Some(&binary), &installed)
        .expect_err("an apply adding an executable to an install");
    assert!(
        matches!(error, InstallError::BinaryContextChanged),
        "{error:?}"
    );
    assert!(!repository.common_dir().join(BINARY_PATH).exists());
}
