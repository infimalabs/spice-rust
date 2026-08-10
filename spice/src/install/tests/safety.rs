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
        planned.operations().len() - 1 - INTERRUPTION
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
