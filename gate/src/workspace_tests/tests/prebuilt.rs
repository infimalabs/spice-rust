use super::*;

#[test]
fn prebuilt_selected_tests_execute_directly_under_bounded_lanes() {
    let root = fixture_root("workspace-prebuilt-direct");
    fs::create_dir_all(&root).expect("prebuilt fixture root creates");
    let arguments_log = root.join("arguments.log");
    let first = executable(
        &root,
        "first-test",
        concat!(
            "#!/bin/sh\n",
            "printf 'first:%s\\n' \"$*\" >> \"$PID_ROOT/arguments.log\"\n",
            "sleep 0.1\n",
            "printf 'test first::done ... ok\\n'\n",
        ),
    );
    let second = executable(
        &root,
        "second-test",
        concat!(
            "#!/bin/sh\n",
            "printf 'second:%s\\n' \"$*\" >> \"$PID_ROOT/arguments.log\"\n",
            "sleep 0.1\n",
            "printf 'test second::done ... ok\\n'\n",
        ),
    );
    let environment = Environment::new([
        (OsString::from("PID_ROOT"), root.clone().into_os_string()),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ])
    .expect("prebuilt environment admits");
    let policy = RunPolicy::production_budgeted(
        WorkspaceTestBudget::new(2, 1, 2).expect("prebuilt budget admits"),
    );
    let package = quick_package();
    let tests = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, executable)| PrebuiltTest {
            package: package.name.clone(),
            package_root: root.clone(),
            target: TargetIdentity {
                name: format!("direct-{index}"),
                ..package.targets[0].clone()
            },
            executable,
            partition: None,
        })
        .collect();
    let mut output = Vec::new();

    Coordinator::new_prebuilt(
        tests,
        root.clone(),
        environment,
        OsString::from("unused-cargo"),
        policy,
        &mut output,
    )
    .run()
    .expect("direct prebuilt shards succeed");

    let mut arguments = fs::read_to_string(arguments_log)
        .expect("prebuilt arguments read")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    arguments.sort();
    assert_eq!(
        arguments,
        [
            "first:--test-threads=2".to_owned(),
            "second:--test-threads=2".to_owned(),
        ]
    );
    let output = String::from_utf8(output).expect("prebuilt progress is UTF-8");
    assert!(output.contains("peak_concurrency=2"), "{output}");
    fs::remove_dir_all(root).expect("prebuilt fixture removes");
}

#[test]
fn prebuilt_partitions_cover_every_case_once_with_exact_complements() {
    let package = quick_package();
    let test = PrebuiltTest {
        package: package.name,
        package_root: PathBuf::from("/fixture"),
        target: package.targets[0].clone(),
        executable: PathBuf::from("/fixture/test"),
        partition: None,
    };
    let cases = (0..129)
        .map(|index| format!("suite::case_{index:03}"))
        .collect::<Vec<_>>();

    let partitions = test.into_partitions(&cases, 3);

    assert_eq!(partitions.len(), 3);
    for case in &cases {
        assert_eq!(
            partitions
                .iter()
                .filter(|partition| {
                    !partition
                        .partition
                        .as_ref()
                        .expect("split test has partition metadata")
                        .skipped
                        .contains(case)
                })
                .count(),
            1,
            "{case} must execute in exactly one complement",
        );
    }
    for partition in partitions {
        let arguments = partition.direct_arguments(2);
        assert_eq!(arguments.first(), Some(&OsString::from("--exact")));
        assert_eq!(arguments.last(), Some(&OsString::from("--test-threads=1")),);
    }
}

#[test]
fn named_exclusive_test_partition_is_a_barrier_between_parallel_test_lanes() {
    let root = fixture_root("workspace-prebuilt-bin-barrier");
    fs::create_dir_all(&root).expect("binary barrier fixture root creates");
    let binary = executable(
        &root,
        "binary-test",
        "#!/bin/sh\nsleep 0.1\nprintf 'test binary::done ... ok\\n'\n",
    );
    let library = executable(
        &root,
        "library-test",
        "#!/bin/sh\nprintf 'test library::done ... ok\\n'\n",
    );
    let environment = Environment::new([(OsString::from("PATH"), OsString::from("/usr/bin:/bin"))])
        .expect("binary barrier environment admits");
    let policy = RunPolicy::production_budgeted(
        WorkspaceTestBudget::new(2, 1, 1).expect("binary barrier budget admits"),
    );
    let package = quick_package();
    let tests = vec![
        PrebuiltTest {
            package: package.name.clone(),
            package_root: root.clone(),
            target: TargetIdentity {
                kind: "bin".to_owned(),
                name: "exclusive".to_owned(),
                ..package.targets[0].clone()
            },
            executable: binary,
            partition: Some(PrebuiltTestPartition {
                index: 0,
                count: 2,
                skipped: vec!["suite::ordinary".to_owned()],
                exclusive: true,
            }),
        },
        PrebuiltTest {
            package: package.name,
            package_root: root.clone(),
            target: TargetIdentity {
                name: "parallel".to_owned(),
                ..package.targets[0].clone()
            },
            executable: library,
            partition: None,
        },
    ];
    let mut output = Vec::new();

    Coordinator::new_prebuilt(
        tests,
        root.clone(),
        environment,
        OsString::from("unused-cargo"),
        policy,
        &mut output,
    )
    .run()
    .expect("binary barrier shards succeed");

    let output = String::from_utf8(output).expect("binary barrier progress is UTF-8");
    let binary_terminal = output
        .find("stage=terminal shard=1/2")
        .expect("binary barrier reaches terminal");
    let library_start = output
        .find("stage=start shard=2/2")
        .expect("library starts after binary barrier");
    assert!(binary_terminal < library_start, "{output}");
    fs::remove_dir_all(root).expect("binary barrier fixture removes");
}

#[test]
fn exclusive_lane_named_case_isolated_from_parallel_partitions() {
    let package = quick_package();
    let test = PrebuiltTest {
        package: package.name,
        package_root: PathBuf::from("/fixture"),
        target: package.targets[0].clone(),
        executable: PathBuf::from("/fixture/test"),
        partition: None,
    };
    let exclusive = "suite::exclusive_lane_observes_process_timing".to_owned();
    let cases = [
        "suite::ordinary_a".to_owned(),
        exclusive.clone(),
        "suite::ordinary_b".to_owned(),
        "suite::ordinary_c".to_owned(),
    ];

    let partitions = test.into_partitions(&cases, 3);

    assert_eq!(partitions.len(), 3);
    let exclusive_partition = partitions
        .iter()
        .find(|partition| {
            partition
                .partition
                .as_ref()
                .is_some_and(|partition| partition.exclusive)
        })
        .expect("exclusive partition exists");
    let metadata = exclusive_partition
        .partition
        .as_ref()
        .expect("exclusive partition has metadata");
    assert!(!metadata.skipped.contains(&exclusive));
    assert!(
        cases
            .iter()
            .filter(|case| *case != &exclusive)
            .all(|case| metadata.skipped.contains(case))
    );
    assert!(
        partitions
            .iter()
            .filter(|partition| {
                !partition
                    .partition
                    .as_ref()
                    .expect("split test has metadata")
                    .skipped
                    .contains(&exclusive)
            })
            .count()
            == 1
    );
}

#[test]
fn selected_prebuild_plan_refuses_a_missing_test_artifact() {
    let inventory = fixture_inventory();
    let message = serde_json::json!({
        "reason": "compiler-artifact",
        "package_id": "fixture-a-id",
        "target": { "name": "fixture-a", "kind": ["lib"] },
        "profile": { "test": true },
        "executable": "/missing/fixture-a",
    });

    let error = prebuilt_test_plan(&inventory, format!("{message}\n").as_bytes())
        .expect_err("missing compiled artifact refuses");

    assert!(error.to_string().contains("missing or not executable"));
}
