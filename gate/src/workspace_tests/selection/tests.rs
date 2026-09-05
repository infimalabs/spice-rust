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
fn representative_change_runs_each_bounded_target_once() {
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
    assert_eq!(maximum_shards, report.tests().len());
    assert_eq!(selected_partitions_per_target(&changes, 3), 1);
    assert_eq!(
        selected_partitions_per_target(&["Cargo.toml".to_owned()], 3),
        3
    );
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

#[test]
fn canvas_private_scheduler_changes_stay_in_canvas_tests_but_retain_downstream_checks() {
    let root = workspace_root();
    let inventory = canvas_scheduler_inventory(&root);
    let changes = [
        "canvas/src/axum/control.rs".to_owned(),
        "canvas/src/axum/tests/lifecycle.rs".to_owned(),
    ];
    let report = selection_plan(
        &inventory,
        &root,
        wire::Digest::compute(wire::Domain::GateSelection, b"canvas-scheduler-change"),
        &changes,
    )
    .expect("Canvas scheduler selection is valid")
    .into_report();

    assert_eq!(report.tests(), ["canvas::lib:canvas"]);
    assert_eq!(
        report
            .edges()
            .iter()
            .map(|edge| (edge.source(), edge.test()))
            .collect::<Vec<_>>(),
        [
            ("canvas/src/axum/control.rs", "canvas::lib:canvas"),
            ("canvas/src/axum/tests/lifecycle.rs", "canvas::lib:canvas",),
        ],
    );

    let checks = cargo_check_selections(&inventory, &root, &changes)
        .expect("internal library changes retain reverse-dependent typechecks")
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
            "canvas::lib:canvas".to_owned(),
            "spice::bin:spice".to_owned(),
            "spice::lib:spice".to_owned(),
            "xtask::bin:xtask".to_owned(),
            "xtask::lib:xtask".to_owned(),
        ]),
    );
}

#[test]
fn canvas_public_module_changes_retain_cross_package_contract_tests() {
    let root = workspace_root();
    let inventory = canvas_scheduler_inventory(&root);
    assert_canvas_public_source_reaches_dependents(
        &inventory,
        &root,
        "canvas/src/axum.rs",
        b"canvas-public-module-change",
    );
}

#[test]
fn canvas_public_reexport_changes_retain_cross_package_contract_tests() {
    let root = workspace_root();
    let inventory = canvas_scheduler_inventory(&root);
    assert_canvas_public_source_reaches_dependents(
        &inventory,
        &root,
        "canvas/src/asset.rs",
        b"canvas-public-reexport-change",
    );
}

#[test]
fn unclassified_library_sources_conservatively_retain_cross_package_tests() {
    let root = workspace_root();
    let inventory = canvas_scheduler_inventory(&root);
    assert_canvas_public_source_reaches_dependents(
        &inventory,
        &root,
        "canvas/src/unclassified.rs",
        b"canvas-unclassified-source-change",
    );
}

#[test]
fn canvas_library_root_changes_retain_cross_package_contract_tests() {
    let root = workspace_root();
    let inventory = canvas_scheduler_inventory(&root);
    assert_canvas_public_source_reaches_dependents(
        &inventory,
        &root,
        "canvas/src/lib.rs",
        b"canvas-contract-change",
    );
}

fn assert_canvas_public_source_reaches_dependents(
    inventory: &Inventory,
    root: &Path,
    changed: &str,
    digest: &[u8],
) {
    let changes = [changed.to_owned()];
    let report = selection_plan(
        inventory,
        root,
        wire::Digest::compute(wire::Domain::GateSelection, digest),
        &changes,
    )
    .expect("Canvas public source selection is valid")
    .into_report();

    assert_eq!(
        report.tests(),
        [
            "canvas::lib:canvas",
            "spice::bin:spice",
            "spice::lib:spice",
            "xtask::bin:xtask",
            "xtask::lib:xtask",
        ],
    );
    assert_eq!(report.edges().len(), 5);
    assert!(report.edges().iter().all(|edge| edge.source() == changed),);
}

#[test]
fn deleted_auto_target_uses_exact_base_owner_and_one_current_validation_unit() {
    let selected_root = synthetic_root("deleted-selected");
    let base_root = synthetic_root("deleted-base");
    let mut selected = deletion_inventory(&selected_root, false);
    let base = deletion_inventory(&base_root, true);
    let changes = ["sample/tests/removed.rs".to_owned()];

    selected
        .retain_removed_target_ownership(&selected_root, &base, &base_root, &changes)
        .expect("the exact base target root establishes deletion ownership");
    assert_eq!(
        selected.packages[0]
            .removed_targets
            .iter()
            .map(TargetIdentity::display)
            .collect::<Vec<_>>(),
        ["test:removed"],
    );
    assert_eq!(
        selected.packages[0].removed_targets[0].source,
        selected_root.join("sample/tests/removed.rs"),
    );

    let report = selection_plan(
        &selected,
        &selected_root,
        wire::Digest::compute(wire::Domain::GateSelection, b"deleted-auto-target"),
        &changes,
    )
    .expect("a deleted integration target has one current validation path")
    .into_report();
    assert_eq!(report.tests(), ["sample::lib:sample"]);
    assert_eq!(
        report
            .edges()
            .iter()
            .map(|edge| (edge.source(), edge.test()))
            .collect::<Vec<_>>(),
        [("sample/tests/removed.rs", "sample::lib:sample")],
    );

    let checks = cargo_check_selections(&selected, &selected_root, &changes)
        .expect("native selection uses the same deletion validation path");
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].targets.len(), 1);
    assert_eq!(checks[0].targets[0].identity, "lib:sample");
}

#[test]
fn deleted_target_without_base_owner_refuses_instead_of_expanding_package() {
    let selected_root = synthetic_root("deleted-unowned");
    let selected = deletion_inventory(&selected_root, false);
    let error = selection_plan(
        &selected,
        &selected_root,
        wire::Digest::compute(wire::Domain::GateSelection, b"unowned-deleted-target"),
        &["sample/tests/removed.rs".to_owned()],
    )
    .expect_err("an absent current target has no conventional-path fallback");

    assert!(
        error
            .to_string()
            .contains("neither current nor base-tree Cargo target ownership establishes it"),
        "unexpected refusal: {error}",
    );
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
        resolution: CargoResolution::default(),
        packages: vec![
            PackageInventory {
                id: "steer".to_owned(),
                name: "steer".to_owned(),
                root: steer_root.clone(),
                dependencies: Vec::new(),
                compile_time_authority: None,
                declared_targets: steer_targets.clone(),
                removed_targets: Vec::new(),
                targets: steer_targets,
            },
            PackageInventory {
                id: "spice".to_owned(),
                name: "spice".to_owned(),
                root: spice_root,
                dependencies: vec![steer_root],
                compile_time_authority: None,
                declared_targets: spice_targets.clone(),
                removed_targets: Vec::new(),
                targets: spice_targets,
            },
        ],
        binary_outputs: Vec::new(),
        target_directory: root.join("target"),
        build_directory: root.join("target/cargo-build"),
    }
}

fn canvas_scheduler_inventory(root: &Path) -> Inventory {
    let canvas_root = root.join("canvas");
    let spice_root = root.join("spice");
    let xtask_root = root.join("xtask");
    let canvas_targets = vec![target(
        &canvas_root,
        "canvas",
        "lib",
        "canvas",
        "src/lib.rs",
    )];
    let spice_targets = vec![
        target(&spice_root, "spice", "lib", "spice", "src/lib.rs"),
        target(&spice_root, "spice", "bin", "spice", "src/main.rs"),
    ];
    let xtask_targets = vec![
        target(&xtask_root, "xtask", "lib", "xtask", "src/lib.rs"),
        target(&xtask_root, "xtask", "bin", "xtask", "src/main.rs"),
    ];
    Inventory {
        resolution: CargoResolution::default(),
        packages: vec![
            PackageInventory {
                id: "canvas".to_owned(),
                name: "canvas".to_owned(),
                root: canvas_root.clone(),
                dependencies: Vec::new(),
                compile_time_authority: None,
                declared_targets: canvas_targets.clone(),
                removed_targets: Vec::new(),
                targets: canvas_targets,
            },
            PackageInventory {
                id: "spice".to_owned(),
                name: "spice".to_owned(),
                root: spice_root.clone(),
                dependencies: vec![canvas_root],
                compile_time_authority: None,
                declared_targets: spice_targets.clone(),
                removed_targets: Vec::new(),
                targets: spice_targets,
            },
            PackageInventory {
                id: "xtask".to_owned(),
                name: "xtask".to_owned(),
                root: xtask_root,
                dependencies: vec![spice_root],
                compile_time_authority: None,
                declared_targets: xtask_targets.clone(),
                removed_targets: Vec::new(),
                targets: xtask_targets,
            },
        ],
        binary_outputs: Vec::new(),
        target_directory: root.join("target"),
        build_directory: root.join("target/cargo-build"),
    }
}

fn deletion_inventory(root: &Path, with_removed: bool) -> Inventory {
    let package_root = root.join("sample");
    let mut targets = vec![target(
        &package_root,
        "sample-id",
        "lib",
        "sample",
        "src/lib.rs",
    )];
    for name in ["kept-a", "kept-b"] {
        targets.push(target(
            &package_root,
            "sample-id",
            "test",
            name,
            &format!("tests/{name}.rs"),
        ));
    }
    if with_removed {
        targets.push(target(
            &package_root,
            "base-sample-id",
            "test",
            "removed",
            "tests/removed.rs",
        ));
    }
    targets.sort();
    Inventory {
        resolution: CargoResolution::default(),
        packages: vec![PackageInventory {
            id: if with_removed {
                "base-sample-id".to_owned()
            } else {
                "sample-id".to_owned()
            },
            name: "sample".to_owned(),
            root: package_root,
            dependencies: Vec::new(),
            compile_time_authority: None,
            declared_targets: targets.clone(),
            removed_targets: Vec::new(),
            targets,
        }],
        binary_outputs: Vec::new(),
        target_directory: root.join("target"),
        build_directory: root.join("target/cargo-build"),
    }
}

fn synthetic_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("gate-selection-{label}-{}", std::process::id()))
}

fn target(root: &Path, package: &str, kind: &str, name: &str, source: &str) -> TargetIdentity {
    let source = root.join(source);
    TargetIdentity {
        package: package.to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
        required_features: Vec::new(),
        declared_path_sources: declared_path_sources(root.parent().unwrap_or(root), &source)
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
