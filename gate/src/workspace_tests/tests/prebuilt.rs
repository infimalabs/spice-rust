use super::*;
use std::sync::Condvar;

const MANY_SELECTED_PACKAGES: usize = 12;
const TARGETS_PER_SELECTED_PACKAGE: usize = 3;
const MANY_SELECTED_TARGETS: usize = MANY_SELECTED_PACKAGES * TARGETS_PER_SELECTED_PACKAGE;
const SELECTED_PREBUILD_LANES: usize = SELECTED_PREBUILD_CONCURRENCY;

#[derive(Default)]
struct PrebuildLaunchState {
    active: usize,
    peak: usize,
    release: bool,
}

#[test]
fn many_selected_targets_prebuild_in_bounded_exact_lanes() {
    let root = fixture_root("workspace-many-selected-prebuild");
    fs::create_dir_all(&root).expect("many-target prebuild fixture root creates");
    let selected = many_target_selected_inventory(&root);
    let batches = selected_prebuild_batches(&selected).expect("exact prebuild batches render");
    assert_many_target_commands_are_exact(&selected, &batches);
    let artifacts = many_target_artifacts(&root);

    let (messages, output, peak) = run_many_target_prebuild(&batches, &artifacts);

    assert_eq!(peak, SELECTED_PREBUILD_LANES);
    let plan = prebuilt_test_plan(&selected, &messages)
        .expect("every selected artifact has complete exact coverage");
    assert_eq!(plan.len(), MANY_SELECTED_TARGETS);
    assert!(plan.iter().all(|test| test.target.name != "unrelated"));
    assert!(
        output.contains("stage=selected-prebuild targets=36 batches=12 lanes=2"),
        "{output}",
    );
    fs::remove_dir_all(root).expect("many-target prebuild fixture removes");
}

fn many_target_selected_inventory(root: &Path) -> Inventory {
    let mut packages = (0..MANY_SELECTED_PACKAGES)
        .map(|package_index| {
            let mut package = quick_package();
            package.id = format!("many-id-{package_index:02}");
            package.name = format!("many-{package_index:02}");
            package.root = root.join(&package.name);
            package.targets = (0..TARGETS_PER_SELECTED_PACKAGE)
                .map(|target_index| TargetIdentity {
                    package: package.id.clone(),
                    kind: "test".to_owned(),
                    name: format!("reached-{package_index:02}-{target_index:02}"),
                    required_features: Vec::new(),
                    source: package
                        .root
                        .join(format!("tests/target-{target_index:02}.rs")),
                    declared_path_sources: Vec::new(),
                    source_bytes: 1,
                })
                .collect();
            package.declared_targets = package.targets.clone();
            package
        })
        .collect::<Vec<_>>();
    let mut unrelated = quick_package();
    unrelated.id = "unrelated-id".to_owned();
    unrelated.name = "unrelated".to_owned();
    unrelated.targets[0].package = unrelated.id.clone();
    unrelated.declared_targets = unrelated.targets.clone();
    packages.push(unrelated);
    let inventory = Inventory {
        packages,
        binary_outputs: Vec::new(),
        target_directory: root.join("target"),
        build_directory: root.join("target/cargo-build"),
    };
    let selected_identities = (0..MANY_SELECTED_PACKAGES)
        .flat_map(|package_index| {
            (0..TARGETS_PER_SELECTED_PACKAGE).map(move |target_index| {
                format!(
                    "many-{package_index:02}::test:reached-{package_index:02}-{target_index:02}",
                )
            })
        })
        .collect::<Vec<_>>();
    inventory
        .selected(&selected_identities)
        .expect("Cargo-proven selected inventory admits")
}

fn assert_many_target_commands_are_exact(selected: &Inventory, batches: &[SelectedPrebuildBatch]) {
    assert_eq!(batches.len(), MANY_SELECTED_PACKAGES);
    assert!(
        batches.iter().all(|batch| {
            batch.arguments.windows(2).any(|pair| {
                pair[0] == OsStr::new("--package")
                    && pair[1].to_string_lossy().starts_with("many-id-")
            }) && batch
                .arguments
                .iter()
                .filter(|argument| *argument == "--test")
                .count()
                == TARGETS_PER_SELECTED_PACKAGE
                && batch
                    .arguments
                    .iter()
                    .all(|argument| argument != "--workspace" && argument != "--all-targets")
                && !batch
                    .arguments
                    .iter()
                    .any(|argument| argument == "unrelated")
        }),
        "every launched Cargo unit stays package-and-target exact",
    );
    let mut published = Vec::new();
    write_selected_test_units(selected, &mut published).expect("selected direct commands publish");
    let published = String::from_utf8(published).expect("published commands are UTF-8");
    assert_eq!(published.lines().count(), MANY_SELECTED_TARGETS);
    assert!(!published.contains("unrelated"), "{published}");
    assert!(!published.contains("--workspace"), "{published}");
    assert!(!published.contains("--all-targets"), "{published}");
}

fn many_target_artifacts(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut artifacts = BTreeMap::new();
    for package_index in 0..MANY_SELECTED_PACKAGES {
        for target_index in 0..TARGETS_PER_SELECTED_PACKAGE {
            let name = format!("reached-{package_index:02}-{target_index:02}");
            let executable = executable(root, &format!("{name}-test"), "#!/bin/sh\nexit 0\n");
            artifacts.insert(name, executable);
        }
    }
    artifacts
}

fn run_many_target_prebuild(
    batches: &[SelectedPrebuildBatch],
    artifacts: &BTreeMap<String, PathBuf>,
) -> (Vec<u8>, String, usize) {
    let state = Arc::new((Mutex::new(PrebuildLaunchState::default()), Condvar::new()));
    let release_state = Arc::clone(&state);
    let release = thread::spawn(move || {
        let (lock, ready) = &*release_state;
        let state = lock.lock().expect("launch state locks");
        let (mut state, _) = ready
            .wait_timeout_while(state, Duration::from_secs(2), |state| {
                state.active < SELECTED_PREBUILD_LANES
            })
            .expect("launch observation waits");
        let filled_every_lane = state.active == SELECTED_PREBUILD_LANES;
        state.release = true;
        ready.notify_all();
        filled_every_lane
    });
    let mut output = Vec::new();
    let messages =
        run_selected_prebuild_batches(batches, SELECTED_PREBUILD_LANES, &mut output, |batch| {
            many_target_prebuild_result(batch, artifacts, &state)
        })
        .expect("many exact selected prebuilds complete");
    assert!(
        release.join().expect("launch observer joins"),
        "the selected prebuild never filled its bounded lanes",
    );
    let launch = state.0.lock().expect("launch state locks");
    assert_eq!(launch.active, 0);
    let peak = launch.peak;

    let output = String::from_utf8(output).expect("prebuild progress is UTF-8");
    (messages, output, peak)
}

fn many_target_prebuild_result(
    batch: &SelectedPrebuildBatch,
    artifacts: &BTreeMap<String, PathBuf>,
    state: &Arc<(Mutex<PrebuildLaunchState>, Condvar)>,
) -> SelectedPrebuildResult {
    let package_id = batch
        .arguments
        .windows(2)
        .find_map(|pair| (pair[0] == OsStr::new("--package")).then(|| pair[1].clone()))
        .expect("exact package selector has an identity")
        .into_string()
        .expect("fixture package identity is UTF-8");
    let (lock, ready) = &**state;
    let mut launch = lock.lock().expect("launch state locks");
    launch.active += 1;
    launch.peak = launch.peak.max(launch.active);
    ready.notify_all();
    launch = ready
        .wait_while(launch, |launch| !launch.release)
        .expect("launch release waits");
    launch.active -= 1;
    drop(launch);

    let messages = batch
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == OsStr::new("--test"))
        .map(|pair| pair[1].clone())
        .map(|name| {
            let name = name.into_string().expect("fixture target name is UTF-8");
            let executable = artifacts
                .get(&name)
                .expect("selected target owns an artifact");
            serde_json::json!({
                "reason": "compiler-artifact",
                "package_id": package_id.clone(),
                "target": { "name": name, "kind": ["test"] },
                "profile": { "test": true },
                "executable": executable,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    SelectedPrebuildResult {
        progress: Vec::new(),
        artifacts: Ok(format!("{messages}\n").into_bytes()),
    }
}

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
