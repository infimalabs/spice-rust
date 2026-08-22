use super::*;

#[test]
fn every_lane_output_authority_is_isolated_by_one_stable_suffix() {
    let environment = Environment::new([
        (
            OsString::from(CARGO_TARGET_DIR),
            OsString::from("/workspace/target"),
        ),
        (
            OsString::from(CARGO_BUILD_BUILD_DIR),
            OsString::from("/workspace/build"),
        ),
    ])
    .expect("base Cargo environment admits");

    let isolated = isolate_shard_outputs(environment, 2).expect("shard isolation admits");
    let entries = isolated
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        entries.get(OsStr::new(CARGO_TARGET_DIR)),
        Some(&OsString::from(
            "/workspace/target/spice-workspace-test-shards/shard-3"
        )),
        "target publications belong to exactly one concurrent shard"
    );
    assert_eq!(
        entries.get(OsStr::new(CARGO_BUILD_BUILD_DIR)),
        Some(&OsString::from(
            "/workspace/build/spice-workspace-test-shards/shard-3"
        )),
        "build publications and their lock belong to the same shard"
    );
}

#[test]
fn sequential_shards_reuse_the_base_output_authority() {
    let root = fixture_root("workspace-lane-reuse");
    fs::create_dir_all(&root).expect("lane reuse fixture root creates");
    let log = root.join("targets.log");
    let cargo = executable(
        &root,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$CARGO_TARGET_DIR\" >> \"$PID_ROOT/targets.log\"\n",
            "printf 'test lane::done ... ok\\n'\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (
            OsString::from(CARGO_TARGET_DIR),
            OsString::from("/workspace/target"),
        ),
        (
            OsString::from(CARGO_BUILD_BUILD_DIR),
            OsString::from("/workspace/build"),
        ),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("lane reuse environment admits");
    let policy = RunPolicy {
        concurrency: 1,
        shared_outputs: false,
        cargo_jobs: None,
        test_threads: None,
        shard_deadline: Duration::from_secs(5),
        coordinator_deadline: Duration::from_secs(10),
        progress_interval: Duration::from_secs(1),
        output_drain: Duration::from_secs(1),
        reap: Duration::from_secs(1),
        grace: Duration::from_millis(50),
    };
    let mut output = Vec::new();

    Coordinator::new(
        vec![quick_package(), quick_package(), quick_package()],
        root.clone(),
        environment,
        cargo.into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect("sequential lane reuse succeeds");

    let targets = fs::read_to_string(&log).expect("lane target log reads");
    assert_eq!(
        targets.lines().collect::<Vec<_>>(),
        [
            "/workspace/target",
            "/workspace/target",
            "/workspace/target",
        ]
    );
    let output = String::from_utf8(output).expect("lane output is UTF-8");
    let lane_starts = output
        .lines()
        .filter(|line| line.contains("stage=start") && line.contains("output_lane=1"))
        .count();
    assert_eq!(lane_starts, 3);
    fs::remove_dir_all(root).expect("lane reuse fixture removes");
}

#[test]
fn shared_output_policy_isolates_publications_but_reuses_the_build_cache() {
    let root = fixture_root("workspace-shared-output-lanes");
    fs::create_dir_all(&root).expect("shared output fixture root creates");
    let log = root.join("targets.log");
    let arguments_log = root.join("arguments.log");
    let cargo = executable(
        &root,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s %s\\n' \"$CARGO_TARGET_DIR\" \"$CARGO_BUILD_BUILD_DIR\" >> ",
            "\"$PID_ROOT/targets.log\"\n",
            "printf '%s\\n' \"$*\" >> \"$PID_ROOT/arguments.log\"\n",
            "printf 'test lane::done ... ok\\n'\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (
            OsString::from(CARGO_TARGET_DIR),
            OsString::from("/workspace/target"),
        ),
        (
            OsString::from(CARGO_BUILD_BUILD_DIR),
            OsString::from("/workspace/build"),
        ),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("shared output environment admits");
    let policy = RunPolicy {
        concurrency: 2,
        shared_outputs: true,
        cargo_jobs: None,
        test_threads: None,
        shard_deadline: Duration::from_secs(5),
        coordinator_deadline: Duration::from_secs(10),
        progress_interval: Duration::from_secs(1),
        output_drain: Duration::from_secs(1),
        reap: Duration::from_secs(1),
        grace: Duration::from_millis(50),
    };
    let mut output = Vec::new();

    Coordinator::new(
        vec![quick_package(), quick_package()],
        root.clone(),
        environment,
        cargo.into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect("parallel shared output lane reuse succeeds");

    let mut targets = fs::read_to_string(&log)
        .expect("shared target log reads")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    targets.sort();
    assert_eq!(
        targets,
        [
            concat!(
                "/workspace/target/spice-workspace-test-shards/shard-1 ",
                "/workspace/build"
            )
            .to_owned(),
            concat!(
                "/workspace/target/spice-workspace-test-shards/shard-2 ",
                "/workspace/build"
            )
            .to_owned(),
        ]
    );
    let expected_harness = format!("-- --test-threads={}", shard_test_threads(&quick_package()));
    let arguments = fs::read_to_string(arguments_log).expect("shared argument log reads");
    assert_eq!(arguments.lines().count(), 2);
    assert!(
        arguments
            .lines()
            .all(|line| line.ends_with(&expected_harness)),
        "shared shards did not bound their test harnesses: {arguments}"
    );
    fs::remove_dir_all(root).expect("shared output fixture removes");
}

#[test]
fn selected_slice_budget_caps_cargo_and_test_harness_parallelism() {
    let root = fixture_root("workspace-selected-budget");
    fs::create_dir_all(&root).expect("selected budget fixture root creates");
    let arguments_log = root.join("arguments.log");
    let cargo = executable(
        &root,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\n' \"$*\" > \"$PID_ROOT/arguments.log\"\n",
            "printf 'test lane::done ... ok\\n'\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("selected budget environment admits");
    let budget = WorkspaceTestBudget::new(2, 1, 1).expect("selected budget admits");
    let policy = RunPolicy::production_budgeted(budget);
    let mut output = Vec::new();

    Coordinator::new(
        vec![quick_package()],
        root.clone(),
        environment,
        cargo.into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect("selected budget execution succeeds");

    let arguments = fs::read_to_string(arguments_log).expect("selected argument log reads");
    assert!(arguments.contains("--jobs 1"), "{arguments}");
    assert!(arguments.ends_with("-- --test-threads=1\n"), "{arguments}");
    fs::remove_dir_all(root).expect("selected budget fixture removes");
}

#[test]
fn sill_process_and_lock_fixtures_use_one_harness_thread() {
    let mut sill = quick_package();
    sill.name = "sill".to_owned();
    assert_eq!(shard_test_threads(&sill), 1);
    assert!((1..=3).contains(&shard_test_threads(&quick_package())));
}

#[test]
fn every_shard_owns_one_private_temporary_root_that_is_reaped_after_exit() {
    let root = fixture_root("workspace-private-temporary-roots");
    fs::create_dir_all(&root).expect("temporary-root fixture creates");
    let log = root.join("temporary-roots.log");
    let cargo = executable(
        &root,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s|%s|%s\\n' \"$TMPDIR\" \"$TMP\" \"$TEMP\" >> ",
            "\"$PID_ROOT/temporary-roots.log\"\n",
            "mkdir \"$TMPDIR/proof\"\n",
            "printf 'test lane::done ... ok\\n'\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (OsString::from("TMPDIR"), OsString::from("/ambient/tmp")),
        (OsString::from("TMP"), OsString::from("/ambient/tmp")),
        (OsString::from("TEMP"), OsString::from("/ambient/tmp")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("temporary-root environment admits");
    let policy = RunPolicy {
        concurrency: 2,
        shared_outputs: true,
        cargo_jobs: None,
        test_threads: None,
        shard_deadline: Duration::from_secs(5),
        coordinator_deadline: Duration::from_secs(10),
        progress_interval: Duration::from_secs(1),
        output_drain: Duration::from_secs(1),
        reap: Duration::from_secs(1),
        grace: Duration::from_millis(50),
    };
    let mut output = Vec::new();

    Coordinator::new(
        vec![quick_package(), quick_package()],
        root.clone(),
        environment,
        cargo.into_os_string(),
        policy,
        &mut output,
    )
    .run()
    .expect("private temporary roots succeed");

    let roots = fs::read_to_string(log)
        .expect("temporary-root log reads")
        .lines()
        .map(|line| line.split('|').map(str::to_owned).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 2);
    assert_ne!(roots[0][0], roots[1][0]);
    for names in roots {
        assert_eq!(names.len(), 3);
        assert_eq!(names[0], names[1]);
        assert_eq!(names[0], names[2]);
        #[cfg(unix)]
        assert!(names[0].starts_with("/tmp/swt-"));
        assert!(!Path::new(&names[0]).exists());
    }
    fs::remove_dir_all(root).expect("temporary-root fixture removes");
}
