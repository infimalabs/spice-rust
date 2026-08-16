//! Stable workspace-test partition and launch-order cases.

use super::*;

#[test]
fn partition_validation_names_omitted_and_duplicated_packages_and_targets() {
    let inventory = fixture_inventory();
    let exact = partition(&inventory);
    validate_partition(&inventory, &exact).expect("exact partition passes");

    let mut omitted = exact.clone();
    omitted[0].targets.pop();
    assert!(
        validate_partition(&inventory, &omitted)
            .expect_err("omitted target refuses")
            .to_string()
            .contains("observed 0")
    );

    let mut duplicated = exact.clone();
    let duplicate = duplicated[0].targets[0].clone();
    duplicated[0].targets.push(duplicate);
    assert!(
        validate_partition(&inventory, &duplicated)
            .expect_err("duplicated target refuses")
            .to_string()
            .contains("observed 2")
    );

    let mut package_missing = exact;
    package_missing.pop();
    let error = validate_partition(&inventory, &package_missing)
        .expect_err("omitted package refuses")
        .to_string();
    assert!(error.contains("package"));
    assert!(error.contains("observed 0"));

    let mut wrong_owner = partition(&inventory);
    wrong_owner[0].targets[0].package = wrong_owner[1].id.clone();
    assert!(
        validate_partition(&inventory, &wrong_owner)
            .expect_err("a target assigned to another package refuses")
            .to_string()
            .contains("target")
    );
}

#[test]
fn partition_balances_large_packages_in_stable_target_order() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets = (0..TARGETS_PER_SHARD + 1)
        .map(|index| TargetIdentity {
            package: "fixture-a-id".to_owned(),
            kind: "test".to_owned(),
            name: format!("target-{index:02}"),
            source: PathBuf::from(format!("/workspace/tests/target-{index:02}.rs")),
            declared_path_sources: Vec::new(),
            source_bytes: 1,
        })
        .collect();

    let shards = partition(&inventory);

    assert_eq!(shards.len(), 3);
    assert_eq!(shards[0].targets.len(), 5);
    assert_eq!(shards[1].targets.len(), 4);
    assert_eq!(
        shards[0]
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>(),
        [
            "target-00",
            "target-02",
            "target-04",
            "target-06",
            "target-08"
        ]
    );
    assert_eq!(
        shards[1]
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>(),
        ["target-01", "target-03", "target-05", "target-07"]
    );
    assert_eq!(shards[2].name, "fixture-b");
    validate_partition(&inventory, &shards).expect("split partition remains exact");
}

#[test]
fn full_verifier_partition_uses_finer_exact_work_units() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets = (0..TARGETS_PER_SHARD + 1)
        .map(|index| TargetIdentity {
            package: "fixture-a-id".to_owned(),
            kind: "test".to_owned(),
            name: format!("target-{index:02}"),
            source: PathBuf::from(format!("/workspace/tests/target-{index:02}.rs")),
            declared_path_sources: Vec::new(),
            source_bytes: u64::try_from(index + 1).expect("fixture weight fits"),
        })
        .collect();

    let ordinary = partition(&inventory);
    let full_verifier = partition_with_target_limit(&inventory, FULL_VERIFIER_TARGETS_PER_SHARD);

    assert_eq!(ordinary.len(), 3);
    assert_eq!(
        full_verifier.len(),
        inventory
            .packages
            .iter()
            .map(|package| package.targets.len())
            .sum::<usize>(),
    );
    assert!(
        full_verifier
            .iter()
            .all(|shard| shard.targets.len() <= FULL_VERIFIER_TARGETS_PER_SHARD)
    );
    assert_eq!(full_verifier[0].targets[0].name, "target-08");
    validate_partition_with_target_limit(
        &inventory,
        &full_verifier,
        FULL_VERIFIER_TARGETS_PER_SHARD,
    )
    .expect("finer full-verifier partition remains exact");
    assert_eq!(
        format!("{full_verifier:#?}"),
        format!(
            "{:#?}",
            partition_with_target_limit(&inventory, FULL_VERIFIER_TARGETS_PER_SHARD)
        ),
        "the finer partition remains deterministic"
    );
}

#[test]
fn partition_spreads_large_source_targets_across_the_same_shard_count() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets = [900, 800, 700, 600, 500, 400, 300, 200, 100]
        .into_iter()
        .enumerate()
        .map(|(index, source_bytes)| TargetIdentity {
            package: "fixture-a-id".to_owned(),
            kind: "test".to_owned(),
            name: format!("target-{index:02}"),
            source: PathBuf::from(format!("/workspace/tests/target-{index:02}.rs")),
            declared_path_sources: Vec::new(),
            source_bytes,
        })
        .collect();

    let shards = partition(&inventory);

    assert_eq!(shards.len(), 3);
    assert_eq!(shards[0].targets.len(), 5);
    assert_eq!(shards[1].targets.len(), 4);
    assert_eq!(
        shards[0]
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>(),
        [
            "target-00",
            "target-03",
            "target-04",
            "target-07",
            "target-08"
        ]
    );
    assert_eq!(
        shards[1]
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>(),
        ["target-01", "target-02", "target-05", "target-06"]
    );
    validate_partition(&inventory, &shards).expect("weighted partition remains exact");
}

#[test]
fn partition_starts_larger_shards_before_metadata_order() {
    let mut inventory = fixture_inventory();
    inventory.packages[1].targets = (0..3)
        .map(|index| TargetIdentity {
            package: "fixture-b-id".to_owned(),
            kind: "test".to_owned(),
            name: format!("target-{index}"),
            source: PathBuf::from(format!("/workspace/fixture-b/tests/target-{index}.rs")),
            declared_path_sources: Vec::new(),
            source_bytes: 1,
        })
        .collect();

    let shards = partition(&inventory);

    assert_eq!(shards[0].name, "fixture-b");
    assert_eq!(shards[0].targets.len(), 3);
    assert_eq!(shards[1].name, "fixture-a");
    validate_partition(&inventory, &shards).expect("heavy-first partition remains exact");
}

#[test]
fn partition_starts_equal_shards_from_the_larger_package_first() {
    let mut inventory = fixture_inventory();
    let targets = |package: &str, count| {
        (0..count)
            .map(|index| TargetIdentity {
                package: package.to_owned(),
                kind: "test".to_owned(),
                name: format!("target-{index:02}"),
                source: PathBuf::from(format!("/workspace/{package}/target-{index:02}.rs")),
                declared_path_sources: Vec::new(),
                source_bytes: 1,
            })
            .collect::<Vec<_>>()
    };
    inventory.packages[0].targets = targets("fixture-a-id", TARGETS_PER_SHARD);
    inventory.packages[1].targets = targets("fixture-b-id", TARGETS_PER_SHARD * 2);

    let shards = partition(&inventory);

    assert_eq!(shards.len(), 3);
    assert_eq!(
        shards
            .iter()
            .map(|shard| shard.name.as_str())
            .collect::<Vec<_>>(),
        ["fixture-b", "fixture-b", "fixture-a"]
    );
    validate_partition(&inventory, &shards).expect("larger-package tie order remains exact");
}

#[test]
fn partition_keeps_startup_dominated_tail_batches_in_metadata_order() {
    let mut inventory = fixture_inventory();
    inventory.packages[1].targets.push(TargetIdentity {
        package: "fixture-b-id".to_owned(),
        kind: "test".to_owned(),
        name: "second".to_owned(),
        source: PathBuf::from("/workspace/fixture-b/tests/second.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    });

    let shards = partition(&inventory);

    assert_eq!(shards[0].name, "fixture-a");
    assert_eq!(shards[0].targets.len(), 1);
    assert_eq!(shards[1].name, "fixture-b");
    assert_eq!(shards[1].targets.len(), 2);
    validate_partition(&inventory, &shards).expect("startup-dominated tail remains exact");
}
