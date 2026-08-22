//! Workspace-test worker budgets and Cargo output authorities.

use super::*;

#[test]
fn production_policy_uses_only_the_scheduler_granted_shard_workers() {
    let production = RunPolicy::production(1).expect("one scheduler grant admits");
    assert_eq!(production.shard_deadline, Duration::from_secs(180));
    assert_eq!(production.coordinator_deadline, Duration::from_secs(255));
    for concurrency in 1..=WORKSPACE_TEST_CONCURRENCY_CAP {
        assert_eq!(
            RunPolicy::production(concurrency)
                .expect("every bounded scheduler grant admits")
                .concurrency,
            concurrency
        );
    }
    assert_eq!(
        RunPolicy::production(WORKSPACE_TEST_CONCURRENCY_CAP)
            .expect("bounded scheduler grant admits")
            .for_shards(2)
            .concurrency,
        2,
        "the coordinator never creates more workers than actual shards"
    );
    for concurrency in [0, WORKSPACE_TEST_CONCURRENCY_CAP + 1] {
        let error = RunPolicy::production(concurrency)
            .expect_err("out-of-contract scheduler grant refuses")
            .to_string();
        assert!(
            error.contains(&format!(
                "must be between 1 and {WORKSPACE_TEST_CONCURRENCY_CAP}"
            )),
            "{error}"
        );
    }
}

#[test]
fn shared_prebuild_policy_alone_admits_the_higher_process_lane_cap() {
    for concurrency in 1..=crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP {
        let policy = RunPolicy::production_shared(concurrency)
            .expect("every shared-prebuild scheduler grant admits");
        assert_eq!(policy.concurrency, concurrency);
        assert!(policy.shared_outputs);
    }
    let error = RunPolicy::production_shared(crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP + 1)
        .expect_err("an oversized shared-prebuild scheduler grant refuses")
        .to_string();
    assert!(
        error.contains(&format!(
            "must be between 1 and {}",
            crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP
        )),
        "{error}"
    );
}

#[test]
fn selected_prebuild_odd_grants_stay_within_the_aggregate_budget() {
    let budget = WorkspaceTestBudget::new(3, 1, 1).expect("odd selected budget admits");
    let grants = budget.prebuild_job_grants(2);
    assert_eq!(grants, [2, 1]);
    assert_eq!(grants.iter().sum::<usize>(), 3);
    let environment = Environment::new([
        (OsString::from("PATH"), OsString::from("/fixture/bin")),
        (OsString::from(CARGO_BUILD_JOBS), OsString::from("1")),
    ])
    .expect("selected environment admits");
    let prebuilds = grants
        .into_iter()
        .map(|jobs| {
            with_cargo_build_jobs(&environment, jobs).expect("per-lane prebuild budget binds")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        prebuilds
            .iter()
            .map(|prebuild| environment_value(prebuild, CARGO_BUILD_JOBS))
            .collect::<Vec<_>>(),
        [Some(&OsString::from("2")), Some(&OsString::from("1"))],
    );
    assert!(prebuilds.iter().all(|prebuild| {
        environment_value(prebuild, "PATH") == Some(&OsString::from("/fixture/bin"))
    }));
}

#[test]
fn selected_prebuild_even_grants_retain_full_bounded_capacity() {
    let budget = WorkspaceTestBudget::new(4, 2, 1).expect("even selected budget admits");
    let grants = budget.prebuild_job_grants(2);
    assert_eq!(grants, [4, 4]);
    assert_eq!(grants.iter().sum::<usize>(), 8);
}

#[test]
fn selected_prebuild_single_active_lane_retains_the_full_grant() {
    let one_package = WorkspaceTestBudget::new(3, 1, 1).expect("one-package budget admits");
    assert_eq!(one_package.prebuild_job_grants(1), [3]);

    let one_lane = WorkspaceTestBudget::new(1, 3, 1).expect("one-lane budget admits");
    assert_eq!(one_lane.prebuild_job_grants(2), [3]);
}

#[test]
fn selected_test_environment_cannot_inherit_the_parent_git_repository() {
    let mut entries = BTreeMap::from([
        (OsString::from("PATH"), OsString::from("/fixture/bin")),
        (
            OsString::from(CARGO_TARGET_DIR),
            OsString::from("/fixture/target"),
        ),
    ]);
    for variable in GIT_LOCAL_ENVIRONMENT {
        entries.insert(
            OsString::from(variable),
            OsString::from("parent-repository"),
        );
    }
    let environment = without_parent_repository(
        Environment::new(entries).expect("declare inherited hook environment"),
    )
    .expect("selected test environment admits after isolation");
    assert_eq!(
        environment_value(&environment, "PATH"),
        Some(&OsString::from("/fixture/bin")),
        "ordinary process environment survives repository isolation"
    );
    assert_eq!(
        environment_value(&environment, CARGO_TARGET_DIR),
        Some(&OsString::from("/fixture/target")),
        "configured Cargo cache authority survives repository isolation"
    );
    for variable in GIT_LOCAL_ENVIRONMENT {
        assert!(
            environment_value(&environment, variable).is_none(),
            "selected tests retained parent repository variable {variable}"
        );
    }
}

#[test]
fn workspace_test_children_drop_only_the_configured_dispatch_recursion_marker() {
    let environment = without_pre_commit_dispatch(
        Environment::new([
            (
                OsString::from(PRE_COMMIT_SOURCE),
                OsString::from("configured-source"),
            ),
            (OsString::from("PATH"), OsString::from("/fixture/bin")),
            (
                OsString::from(CARGO_TARGET_DIR),
                OsString::from("/fixture/target"),
            ),
            (
                OsString::from("SPICE_RS_PRE_COMMIT_PATHS"), // env-policy: allow
                OsString::from("[\"spice/tests/gate_cli.rs\"]"),
            ),
        ])
        .expect("configured workspace-test environment admits"),
    )
    .expect("workspace-test child environment admits");

    assert!(
        environment_value(&environment, PRE_COMMIT_SOURCE).is_none(),
        "selected Cargo children cannot inherit the configured-command recursion marker"
    );
    assert_eq!(
        environment_value(&environment, "PATH"),
        Some(&OsString::from("/fixture/bin")),
    );
    assert_eq!(
        environment_value(&environment, CARGO_TARGET_DIR),
        Some(&OsString::from("/fixture/target")),
    );
    assert_eq!(
        environment_value(&environment, "SPICE_RS_PRE_COMMIT_PATHS"), // env-policy: allow
        Some(&OsString::from("[\"spice/tests/gate_cli.rs\"]")),
        "the staged-source binding remains available to selected execution"
    );
}

#[test]
fn selected_tree_execution_rewrites_external_cargo_outputs_to_writable_root() {
    let inventory = Inventory {
        target_directory: PathBuf::from("/selected/target"),
        build_directory: PathBuf::from("/machine/cargo-build/hash"),
        binary_outputs: Vec::new(),
        packages: vec![quick_package()],
    };
    let environment = inventory
        .execution_environment(
            Environment::new([(OsString::from("PATH"), OsString::from("/fixture/bin"))])
                .expect("selected test environment admits"),
            Path::new("/selected"),
            Path::new("/worktree"),
            false,
        )
        .expect("selected test output roots admit");

    assert_eq!(
        environment_value(&environment, CARGO_TARGET_DIR),
        Some(&OsString::from("/worktree/target")),
        "ordinary selected-tree tests compile into the writable worktree target"
    );
    assert_eq!(
        environment_value(&environment, CARGO_BUILD_BUILD_DIR),
        Some(&OsString::from("/worktree/target/cargo-build")),
        "machine-level build-dir paths are not embedded into selected-tree test binaries"
    );
}

#[test]
fn reusable_workspace_execution_preserves_prepared_cargo_outputs() {
    let inventory = Inventory {
        target_directory: PathBuf::from("/worktree/target/spice-gate-native"),
        build_directory: PathBuf::from(
            "/worktree/target/spice-gate-native/spice-gate-native-stable-build-v1",
        ),
        binary_outputs: Vec::new(),
        packages: vec![quick_package()],
    };
    let environment = inventory
        .execution_environment(
            Environment::new([(OsString::from("PATH"), OsString::from("/fixture/bin"))])
                .expect("reusable test environment admits"),
            Path::new("/selected"),
            Path::new("/worktree"),
            true,
        )
        .expect("reusable output roots admit");

    assert_eq!(
        environment_value(&environment, CARGO_TARGET_DIR),
        Some(&OsString::from("/worktree/target/spice-gate-native")),
        "reusable test execution keeps the analysis target root"
    );
    assert_eq!(
        environment_value(&environment, CARGO_BUILD_BUILD_DIR),
        Some(&OsString::from(
            "/worktree/target/spice-gate-native/spice-gate-native-stable-build-v1"
        )),
        "reusable test execution keeps the analysis build root"
    );
}

#[test]
fn selected_test_environment_names_the_separate_writable_fixture_root() {
    let environment = fixture_inventory()
        .execution_environment(
            Environment::new(Vec::<(OsString, OsString)>::new())
                .expect("empty fixture environment"),
            Path::new("/workspace"),
            Path::new("/writable"),
            false,
        )
        .expect("selected test environment admits");

    assert_eq!(
        environment_value(&environment, TEST_WRITABLE_ROOT),
        Some(&OsString::from("/writable")),
        "fixtures receive writable authority without changing the selected source root"
    );
}
