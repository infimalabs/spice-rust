use super::*;

#[test]
fn replacement_symlink_and_metadata_drift_survive_pre_unlink_recheck() {
    for (label, mutation) in [("replacement", 0_u8), ("symlink", 1_u8), ("metadata", 2_u8)] {
        let fixture = Fixture::new(label);
        let path = fixture.regular(std::process::id(), 0, 11);
        let runtime = fixture.runtime();
        let inspection = scan(
            &fixture.repository,
            true,
            SystemTime::now() + Duration::from_secs(2 * 60 * 60),
            |_| ProcessState::Dead,
            |path, _| {
                match mutation {
                    0 => {
                        fs::remove_file(path).unwrap();
                        fs::write(path, b"replacement").unwrap();
                        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
                    }
                    1 => {
                        fs::remove_file(path).unwrap();
                        symlink("unrelated", path).unwrap();
                    }
                    2 => {
                        fs::set_permissions(path, fs::Permissions::from_mode(0o400)).unwrap();
                    }
                    _ => unreachable!(),
                }
                Ok(())
            },
        )
        .expect("race is retained");

        assert!(path.exists() || fs::symlink_metadata(&path).is_ok());
        assert!(!inspection.unsafe_candidates().is_empty());
        assert!(runtime.exists());
    }
}

#[test]
fn a_pid_that_becomes_live_during_recheck_is_retained() {
    let fixture = Fixture::new("pid-reuse");
    let path = fixture.regular(std::process::id(), 0, 11);
    let mut probes = 0;
    let inspection = scan(
        &fixture.repository,
        true,
        SystemTime::now() + Duration::from_secs(2 * 60 * 60),
        |_| {
            probes += 1;
            if probes == 1 {
                ProcessState::Dead
            } else {
                ProcessState::Live
            }
        },
        |_, _| Ok(()),
    )
    .expect("PID reuse is retained");

    assert_eq!(probes, 2);
    assert!(path.exists());
    assert!(
        inspection
            .unsafe_candidates()
            .iter()
            .any(|candidate| candidate.contains("eligibility changed before removal"))
    );
}

#[test]
fn concurrent_disappearance_is_benign_and_only_one_cleaner_removes() {
    let fixture = Fixture::new("concurrent");
    let path = fixture.regular(std::process::id(), 0, 17);
    let now = SystemTime::now() + Duration::from_secs(2 * 60 * 60);
    let first = scan(
        &fixture.repository,
        true,
        now,
        |_| ProcessState::Dead,
        |path, _| {
            fs::remove_file(path).unwrap();
            Ok(())
        },
    )
    .expect("concurrent removal is benign");
    assert_eq!(first.reclaimable_count(), 1);
    assert!(!path.exists());

    let second = scan(
        &fixture.repository,
        true,
        now,
        |_| ProcessState::Dead,
        |_, _| Ok(()),
    )
    .expect("second cleaner is empty");
    assert_eq!(second.reclaimable_count(), 0);
}

#[test]
fn scan_and_candidate_caps_report_incomplete_results() {
    let entries = Fixture::new("entry-cap");
    for index in 0..=ENTRY_LIMIT {
        fs::write(entries.runtime().join(format!("foreign-{index}")), b"").unwrap();
    }
    let entry_report = scan(
        &entries.repository,
        false,
        SystemTime::now(),
        |_| ProcessState::Dead,
        |_, _| Ok(()),
    )
    .expect("bounded entry scan");
    assert_eq!(entry_report.examined_entries(), ENTRY_LIMIT);
    assert_eq!(entry_report.omitted_entries(), 1);
    assert!(entry_report.incomplete());

    let candidates = Fixture::new("candidate-cap");
    for counter in 0..=CANDIDATE_LIMIT {
        candidates.regular(std::process::id(), counter as u64, 1);
    }
    let candidate_report = scan(
        &candidates.repository,
        false,
        SystemTime::now(),
        |_| ProcessState::Dead,
        |_, _| Ok(()),
    )
    .expect("bounded candidate scan");
    assert_eq!(candidate_report.omitted_candidates(), 1);
    assert!(candidate_report.incomplete());
}

#[test]
fn absent_runtime_is_not_created_and_asset_names_are_never_scanned() {
    let fixture = Fixture::new("scope");
    fs::remove_dir(fixture.runtime()).expect("remove empty runtime directory");
    let absent = inspect_temporary_records(&fixture.repository).expect("inspect absence");
    assert!(!absent.runtime_present());
    assert!(!fixture.runtime().exists());

    let asset_directory = fixture.root.join(".git/.spice-install/bin");
    fs::create_dir_all(&asset_directory).expect("create asset directory");
    let asset_temporary = asset_directory.join(Fixture::name(std::process::id(), 0));
    fs::write(&asset_temporary, b"asset temporary").expect("write asset temporary");
    fs::set_permissions(&asset_temporary, fs::Permissions::from_mode(0o600)).unwrap();
    let absent = inspect_temporary_records(&fixture.repository).expect("inspect absence again");
    assert!(!absent.runtime_present());
    assert_eq!(fs::read(&asset_temporary).unwrap(), b"asset temporary");
}

#[test]
fn bounded_candidate_evidence_is_sorted_by_raw_basename_bytes() {
    let fixture = Fixture::new("ordering");
    let pid = std::process::id();
    for counter in [9, 2, 10] {
        let path = fixture.regular(pid, counter, 1);
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    let inspection = inspect_temporary_records(&fixture.repository).expect("inspect ordering");
    let names = inspection
        .unsafe_candidates()
        .iter()
        .map(|candidate| candidate.split(':').next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            Fixture::name(pid, 10),
            Fixture::name(pid, 2),
            Fixture::name(pid, 9),
        ]
    );
}

#[test]
fn scan_and_open_boundary_replacements_are_never_unlinked() {
    for (label, action) in [
        ("scan-disappearance", BoundaryAction::RemoveAfterScan),
        ("open-replacement", BoundaryAction::ReplaceBeforeOpen),
        ("open-symlink", BoundaryAction::SymlinkBeforeOpen),
    ] {
        let fixture = Fixture::new(label);
        let path = fixture.regular(std::process::id(), 0, 7);
        let mut hooks = BoundaryHooks {
            action: Some(action),
            target: path.clone(),
            syncs: 0,
        };
        let inspection = scan_with_hooks(
            &fixture.repository,
            true,
            SystemTime::now() + Duration::from_secs(2 * 60 * 60),
            &mut |_| ProcessState::Dead,
            &mut hooks,
        )
        .expect("boundary race is safe");

        if label == "scan-disappearance" {
            assert!(!path.exists());
            assert!(inspection.unsafe_candidates().is_empty());
        } else {
            assert!(path.exists() || fs::symlink_metadata(&path).is_ok());
            assert_eq!(inspection.unsafe_candidates().len(), 1);
        }
        assert_eq!(hooks.syncs, 0);
    }
}

#[test]
fn runtime_directory_sync_is_attempted_only_after_a_successful_unlink() {
    let fresh = Fixture::new("no-sync");
    let fresh_path = fresh.regular(std::process::id(), 0, 7);
    let mut fresh_hooks = BoundaryHooks {
        action: None,
        target: fresh_path.clone(),
        syncs: 0,
    };
    scan_with_hooks(
        &fresh.repository,
        true,
        SystemTime::now(),
        &mut |_| ProcessState::Dead,
        &mut fresh_hooks,
    )
    .expect("fresh candidate is retained");
    assert!(fresh_path.exists());
    assert_eq!(fresh_hooks.syncs, 0);

    let stale = Fixture::new("one-sync");
    let stale_path = stale.regular(std::process::id(), 0, 7);
    let mut stale_hooks = BoundaryHooks {
        action: None,
        target: stale_path.clone(),
        syncs: 0,
    };
    scan_with_hooks(
        &stale.repository,
        true,
        SystemTime::now() + Duration::from_secs(2 * 60 * 60),
        &mut |_| ProcessState::Dead,
        &mut stale_hooks,
    )
    .expect("stale candidate is removed");
    assert!(!stale_path.exists());
    assert_eq!(stale_hooks.syncs, 1);
}

#[test]
fn a_partial_reclamation_is_synchronized_before_its_later_error_returns() {
    let fixture = Fixture::new("partial-sync");
    let first = fixture.regular(std::process::id(), 0, 7);
    let second = fixture.regular(std::process::id(), 1, 7);
    let mut hooks = FailSecondUnlink::default();
    let error = scan_with_hooks(
        &fixture.repository,
        true,
        SystemTime::now() + Duration::from_secs(2 * 60 * 60),
        &mut |_| ProcessState::Dead,
        &mut hooks,
    )
    .expect_err("the injected second-unlink failure is returned");

    assert!(matches!(error, InstallError::InvalidRecord(_)));
    assert!(!first.exists());
    assert!(second.exists());
    assert_eq!(hooks.syncs, 1);
}
