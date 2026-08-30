use super::*;

const REPRESENTATIVE_CHANGES: [&str; 5] = [
    "spice/src/resident/message_surface.rs",
    "spice/src/resident/tests/message_backlog.rs",
    "steer/src/ack/scan.rs",
    "steer/src/lib.rs",
    "steer/tests/scan.rs",
];

#[test]
fn representative_steer_and_resident_change_has_exact_contract_edges() {
    let root = workspace_root();
    let inventory = representative_inventory(&root);
    let changes = REPRESENTATIVE_CHANGES.map(str::to_owned);
    let report = selection_plan(
        &inventory,
        &root,
        wire::Digest::compute(
            wire::Domain::GateSelection,
            b"representative-five-file-change",
        ),
        &changes,
    )
    .expect("representative selection is valid")
    .into_report();

    assert_eq!(
        report.tests(),
        [
            "spice::bin:spice",
            "spice::lib:spice",
            "steer::lib:steer",
            "steer::test:scan",
        ]
    );
    assert_eq!(
        report
            .edges()
            .iter()
            .map(|edge| (edge.source(), edge.test()))
            .collect::<Vec<_>>(),
        [
            ("spice/src/resident/message_surface.rs", "spice::bin:spice",),
            (
                "spice/src/resident/tests/message_backlog.rs",
                "spice::bin:spice",
            ),
            ("steer/src/ack/scan.rs", "spice::bin:spice"),
            ("steer/src/ack/scan.rs", "spice::lib:spice"),
            ("steer/src/ack/scan.rs", "steer::lib:steer"),
            ("steer/src/lib.rs", "spice::bin:spice"),
            ("steer/src/lib.rs", "spice::lib:spice"),
            ("steer/src/lib.rs", "steer::lib:steer"),
            ("steer/tests/scan.rs", "steer::test:scan"),
        ]
    );
    for unrelated in ["cargo_isolation_cli", "install", "steering"] {
        assert!(
            report.tests().iter().all(|test| !test.contains(unrelated)),
            "unrelated process suite {unrelated:?} entered routine selection"
        );
    }

    let checks = cargo_check_selections(&inventory, &root, &changes)
        .expect("representative check selection is valid")
        .into_iter()
        .flat_map(|package| {
            package
                .targets
                .into_iter()
                .map(move |target| format!("{}::{}", package.name, target.identity))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        checks,
        BTreeSet::from([
            "spice::bin:spice".to_owned(),
            "spice::lib:spice".to_owned(),
            "steer::lib:steer".to_owned(),
            "steer::test:scan".to_owned(),
        ])
    );
}

#[test]
fn representative_change_stays_below_historical_target_and_shard_cost() {
    let root = workspace_root();
    let inventory = representative_inventory(&root);
    let changes = REPRESENTATIVE_CHANGES.map(str::to_owned);
    let report = selection_plan(
        &inventory,
        &root,
        wire::Digest::compute(
            wire::Domain::GateSelection,
            b"representative-selection-budget",
        ),
        &changes,
    )
    .expect("representative selection is within the routine budget")
    .into_report();

    assert!(report.tests().len() < 54);
    assert!(report.tests().len() <= ROUTINE_TEST_TARGET_LIMIT);
    let maximum_shards = report.tests().len() * ROUTINE_PARTITIONS_PER_TARGET;
    assert!(maximum_shards < 75);
    assert!(maximum_shards <= ROUTINE_TEST_SHARD_LIMIT);
}

#[test]
fn routine_target_budget_refuses_growth_but_exempts_selection_authority() {
    let error = enforce_routine_target_budget(
        &["steer/src/lib.rs".to_owned()],
        ROUTINE_TEST_TARGET_LIMIT + 1,
    )
    .expect_err("ordinary source reach must remain bounded");
    assert!(
        error
            .to_string()
            .contains("exceeding the repository limit of 32")
    );

    enforce_routine_target_budget(&["Cargo.toml".to_owned()], ROUTINE_TEST_TARGET_LIMIT + 1)
        .expect("workspace authority requires exhaustive selection");
}

#[test]
fn workspace_authority_retains_process_proofs() {
    let root = workspace_root();
    let inventory = representative_inventory(&root);
    let changes = ["Cargo.toml".to_owned()];
    let report = selection_plan(
        &inventory,
        &root,
        wire::Digest::compute(
            wire::Domain::GateSelection,
            b"workspace-authority-selection",
        ),
        &changes,
    )
    .expect("workspace authority remains exhaustive")
    .into_report();

    assert!(!routine_selection(&changes));
    assert_eq!(report.tests().len(), 9);
    for required in ["cargo_isolation_cli", "install", "steering"] {
        assert!(
            report.tests().iter().any(|test| test.contains(required)),
            "workspace authority omitted process suite {required:?}"
        );
    }
}

fn representative_inventory(root: &Path) -> Inventory {
    let steer_root = root.join("steer");
    let spice_root = root.join("spice");
    let steer_targets = vec![
        target(&steer_root, "steer", "lib", "steer", "src/lib.rs"),
        target(&steer_root, "steer", "test", "bound", "tests/bound.rs"),
        target(&steer_root, "steer", "test", "door", "tests/door.rs"),
        target(&steer_root, "steer", "test", "scan", "tests/scan.rs"),
    ];
    let spice_targets = vec![
        target(&spice_root, "spice", "lib", "spice", "src/lib.rs"),
        target(&spice_root, "spice", "bin", "spice", "src/main.rs"),
        target(
            &spice_root,
            "spice",
            "test",
            "cargo_isolation_cli",
            "tests/cargo_isolation_cli.rs",
        ),
        target(&spice_root, "spice", "test", "install", "tests/install.rs"),
        target(
            &spice_root,
            "spice",
            "test",
            "steering",
            "tests/steering.rs",
        ),
    ];
    Inventory {
        packages: vec![
            PackageInventory {
                id: "steer".to_owned(),
                name: "steer".to_owned(),
                root: steer_root.clone(),
                dependencies: Vec::new(),
                declared_targets: steer_targets.clone(),
                targets: steer_targets,
            },
            PackageInventory {
                id: "spice".to_owned(),
                name: "spice".to_owned(),
                root: spice_root,
                dependencies: vec![steer_root],
                declared_targets: spice_targets.clone(),
                targets: spice_targets,
            },
        ],
        binary_outputs: Vec::new(),
        target_directory: root.join("target"),
        build_directory: root.join("target/cargo-build"),
    }
}

fn target(root: &Path, package: &str, kind: &str, name: &str, source: &str) -> TargetIdentity {
    let source = root.join(source);
    TargetIdentity {
        package: package.to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
        required_features: Vec::new(),
        declared_path_sources: declared_path_sources(&source)
            .expect("representative target source graph is valid"),
        source,
        source_bytes: 0,
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate package has a workspace parent")
        .canonicalize()
        .expect("workspace root is canonical")
}
