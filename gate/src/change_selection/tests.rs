use super::*;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-change-selection-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create selector fixture");
        crate::test_repository::initialize(&root);
        Self { root }
    }

    fn stage(&self, path: &str, bytes: &[u8]) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture path parent");
        }
        fs::write(target, bytes).expect("write fixture path");
        crate::test_repository::stage(&self.root, &[path]);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn source() -> Digest {
    Digest::compute(wire::Domain::GateTree, b"change selection fixture")
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn policy<'a>(
    tracked_sources: &'a [String],
    configured: bool,
    suffixes: Option<&'a [String]>,
    package_roots: &'a [String],
    generated_paths: &'a [String],
    test_paths: &'a [String],
) -> ChangeSelectionPolicy {
    ChangeSelectionPolicy::new(ChangeSelectionPolicyInputs {
        tracked_sources,
        policy_authorities: &["spice-rust.toml".to_owned()],
        configured,
        repository_documents: Some(&[]),
        file_shape_suffixes: suffixes,
        complexity_suffixes: suffixes,
        magic_suffixes: suffixes,
        package_roots,
        generated_paths,
        test_paths,
    })
    .expect("compile fixture selection policy")
}

fn selection(paths: &[&str], policy: &ChangeSelectionPolicy) -> ChangeSelectionReport {
    select_change_gates(source(), &strings(paths), policy).expect("select representative changes")
}

fn contains(report: &ChangeSelectionReport, gates: &[GateId]) {
    for gate in gates {
        assert!(
            report.gate_ids().contains(gate),
            "{} is absent from {:?}",
            gate,
            report.gate_ids(),
        );
    }
}

fn assert_selection_authority_exhaustive(report: &ChangeSelectionReport, reason: &str) {
    assert_eq!(report.gate_ids(), report.applicable_gate_ids());
    assert_eq!(
        report.edges().len(),
        report.changed_sources().len() + report.gate_ids().len() - 1,
        "authority escalation proof must grow linearly, not by path-arm product",
    );
    for reach in report.reach() {
        assert_eq!(
            reach.mode(),
            &ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::SelectionPolicy,
                reason: reason.to_owned(),
            }
        );
        assert_eq!(reach.objects(), &[ChangeReachObject::Repository]);
    }
}

#[test]
fn authority_escalation_retains_every_source_in_a_linear_report() {
    let mut changed = (0..64)
        .map(|index| format!("docs/report-{index:02}.md"))
        .collect::<Vec<_>>();
    changed.push("gate/src/change_selection.rs".to_owned());
    changed.sort();

    let report = select_current_source_change_gates(source(), &changed)
        .expect("known authority and document paths select direct exhaustive execution");

    assert_selection_authority_exhaustive(
        &report,
        "compiled current-source selection authority changed",
    );
    for source in &changed {
        assert!(
            report.edges().iter().any(|edge| edge.source() == source),
            "authority escalation dropped changed source {source:?}",
        );
    }
    assert_eq!(
        report
            .reach()
            .iter()
            .map(|reach| reach.provenance().len())
            .sum::<usize>(),
        report.edges().len() + ALWAYS.len(),
    );
}

#[test]
fn a_non_cargo_repository_has_a_deterministic_generic_selection() {
    let scratch = Scratch::new("non-cargo");
    scratch.stage("notes/readme.md", b"portable repository\n");
    scratch.stage("assets/logo.bin", &[0, 1, 2]);
    let tree = crate::Selector::staged()
        .materialize(&scratch.root)
        .expect("capture a non-Cargo staged tree");
    let policy = policy(tree.paths(), false, None, &[], &[], &[]);
    let report = selection(&["notes/readme.md"], &policy);

    assert_eq!(report.mode(), ChangeSelectionMode::Local);
    contains(
        &report,
        &[
            GateId::MergeIntegrity,
            GateId::PlanPhase,
            GateId::Staging,
            GateId::LocalPaths,
            GateId::Taste,
            GateId::MarkdownLinks,
        ],
    );
    assert!(!report.gate_ids().contains(&GateId::RustTypecheck));
    assert!(!report.gate_ids().contains(&GateId::CanvasWebTypecheck));
}

#[test]
fn a_differently_shaped_rust_repository_uses_policy_not_package_names() {
    let tracked = strings(&[
        "Cargo.toml",
        "crates/nebula/Cargo.toml",
        "crates/nebula/src/lib.rs",
        "spice-rust.toml",
    ]);
    let suffixes = strings(&[".rs"]);
    let roots = strings(&["crates/nebula"]);
    let policy = policy(&tracked, true, Some(&suffixes), &roots, &[], &[]);
    let report = selection(&["crates/nebula/src/lib.rs"], &policy);

    assert_eq!(report.mode(), ChangeSelectionMode::Local);
    contains(
        &report,
        &[
            GateId::Formatters,
            GateId::RustTypecheck,
            GateId::FileShape,
            GateId::Complexity,
            GateId::MagicNumbers,
        ],
    );
    let manifest = selection(&["crates/nebula/Cargo.toml"], &policy);
    assert!(manifest.gate_ids().contains(&GateId::RustTypecheck));
    for (path, condition) in [
        (
            "tools/outside.rs",
            "is Rust source outside every compiled workspace root",
        ),
        (
            "tools/outside/src/lib.rs",
            "is Rust source outside every compiled workspace root",
        ),
        (
            "tools/component/Cargo.toml",
            "is a package manifest outside every compiled workspace root",
        ),
    ] {
        let error = select_change_gates(source(), &strings(&[path]), &policy)
            .expect_err("out-of-workspace Rust input must refuse");
        assert_eq!(error.detail(), format!("{path:?} {condition}"));
    }
    for authority in [
        ".cargo/config",
        ".cargo/config.toml",
        "Cargo.lock",
        "rust-toolchain",
        "rust-toolchain.toml",
    ] {
        let report = select_change_gates(source(), &strings(&[authority]), &policy)
            .expect("repository build authorities select direct exhaustive execution");
        assert_selection_authority_exhaustive(
            &report,
            "compiled packaged selection authority changed",
        );
    }
}

#[test]
fn mixed_language_generated_package_and_test_policy_shapes_selection() {
    let tracked = strings(&[
        "pyproject.toml",
        "python/acme/core.py",
        "checks/core_spec.py",
        "web/main.ts",
        "dist/generated.ts",
        "spice-rust.toml",
    ]);
    let suffixes = strings(&[".py", ".ts"]);
    let roots = strings(&["python/acme"]);
    let generated = strings(&["dist"]);
    let tests = strings(&["checks"]);
    let policy = policy(&tracked, true, Some(&suffixes), &roots, &generated, &tests);

    let test = selection(&["checks/core_spec.py"], &policy);
    assert!(test.gate_ids().contains(&GateId::AssertionFreeTests));
    assert!(test.gate_ids().contains(&GateId::PythonTypecheck));
    assert!(!test.gate_ids().contains(&GateId::RepoShape));

    let package = selection(&["python/acme/core.py"], &policy);
    contains(
        &package,
        &[
            GateId::RepoShape,
            GateId::PythonTypecheck,
            GateId::FileShape,
        ],
    );

    let generated = selection(&["dist/generated.ts"], &policy);
    assert_eq!(generated.mode(), ChangeSelectionMode::Local);
    assert_eq!(
        generated.gate_ids(),
        [
            GateId::MergeIntegrity,
            GateId::PlanPhase,
            GateId::Staging,
            GateId::LocalPaths,
        ]
    );
}

#[test]
fn deleting_the_last_configured_source_keeps_its_gate_class_applicable() {
    let tracked = strings(&["Cargo.toml", "spice-rust.toml"]);
    let suffixes = strings(&[".rs"]);
    let policy = policy(&tracked, true, Some(&suffixes), &[], &[], &[]);
    let report = selection(&["src/retired/component.rs"], &policy);

    assert_eq!(report.mode(), ChangeSelectionMode::Local);
    contains(
        &report,
        &[
            GateId::Formatters,
            GateId::RustTypecheck,
            GateId::FileShape,
            GateId::Complexity,
            GateId::MagicNumbers,
        ],
    );
}

#[test]
fn policy_authority_executes_exhaustively_while_unknown_changes_refuse() {
    let tracked = strings(&["assets/opaque.bin", "notes/readme.md", "spice-rust.toml"]);
    let suffixes = strings(&[".md"]);
    let policy = policy(&tracked, true, Some(&suffixes), &[], &[], &[]);
    let authority = select_change_gates(source(), &strings(&["spice-rust.toml"]), &policy)
        .expect("packaged selector authority selects direct exhaustive execution");
    assert_selection_authority_exhaustive(
        &authority,
        "compiled packaged selection authority changed",
    );
    let unknown = select_change_gates(source(), &strings(&["assets/opaque.bin"]), &policy)
        .expect_err("unclassified packaged path must refuse");
    assert_eq!(
        unknown.detail(),
        "\"assets/opaque.bin\" has no repository-declared packaged policy class"
    );
}

#[test]
fn generic_selection_defers_canvas_topology_to_canonical_cargo_reach() {
    let tracked = strings(&[
        "Cargo.toml",
        "app/browser/canvas-coordinator.mjs",
        "journal/Cargo.toml",
        "journal/src/lib.rs",
        "spice-rust.toml",
    ]);
    let suffixes = strings(&[".mjs", ".rs"]);
    let policy = policy(&tracked, true, Some(&suffixes), &[], &[], &[]);
    let report = selection(
        &["app/browser/canvas-coordinator.mjs", "journal/src/lib.rs"],
        &policy,
    );

    assert_eq!(report.mode(), ChangeSelectionMode::Local);
    contains(
        &report,
        &[
            GateId::JavascriptUnused,
            GateId::Formatters,
            GateId::RustTypecheck,
        ],
    );
    assert!(report.gate_ids().contains(&GateId::CanvasWebTypecheck));
}

#[test]
fn configured_policy_missing_a_required_language_class_refuses() {
    let tracked = strings(&["spice-rust.toml", "src/lib.rs"]);
    let suffixes = strings(&[".rs"]);
    let error = ChangeSelectionPolicy::new(ChangeSelectionPolicyInputs {
        tracked_sources: &tracked,
        policy_authorities: &["spice-rust.toml".to_owned()],
        configured: true,
        repository_documents: None,
        file_shape_suffixes: Some(&suffixes),
        complexity_suffixes: None,
        magic_suffixes: Some(&suffixes),
        package_roots: &[],
        generated_paths: &[],
        test_paths: &[],
    })
    .expect_err("malformed policy cannot inherit compiled inventory");

    assert!(error.detail().contains("policy.languages.complexity"));
}

#[test]
fn repository_documents_do_not_activate_the_excluded_file_shape_arm() {
    let tracked = strings(&["HANDBOOK.md", "spice-rust.toml"]);
    let suffixes = strings(&[".md"]);
    let documents = strings(&["HANDBOOK.md"]);
    let policy = ChangeSelectionPolicy::new(ChangeSelectionPolicyInputs {
        tracked_sources: &tracked,
        policy_authorities: &["spice-rust.toml".to_owned()],
        configured: true,
        repository_documents: Some(&documents),
        file_shape_suffixes: Some(&suffixes),
        complexity_suffixes: Some(&suffixes),
        magic_suffixes: Some(&suffixes),
        package_roots: &[],
        generated_paths: &[],
        test_paths: &[],
    })
    .expect("compile repository-document fixture");
    let report = selection(&["HANDBOOK.md"], &policy);

    assert!(report.gate_ids().contains(&GateId::RepoDocs));
    assert!(!report.gate_ids().contains(&GateId::FileShape));
}

#[test]
fn current_source_extensionless_wrappers_select_the_shell_policy_class() {
    let report =
        select_current_source_change_gates(source(), &strings(&["steer/hooks/wrappers/spice"]))
            .expect("the tracked wrapper directory is an extensionless shell class");

    contains(
        &report,
        &[
            GateId::LocalPaths,
            GateId::FileShape,
            GateId::EnvPolicy,
            GateId::EnvNameLedger,
            GateId::Complexity,
            GateId::MagicNumbers,
        ],
    );
}

#[test]
fn current_source_authorities_execute_exhaustively_and_local_reports_are_canonical() {
    for path in [
        "spice-rust.toml",
        "gate/src/change_selection.rs",
        "gate/src/change_selection/policy.rs",
        "spice/src/change_gate.rs",
        "xtask/src/gate_command.rs",
    ] {
        let report = select_current_source_change_gates(source(), &strings(&[path]))
            .expect("selector authority selects direct exhaustive execution");
        assert_selection_authority_exhaustive(
            &report,
            "compiled current-source selection authority changed",
        );
    }

    let report = select_current_source_change_gates(
        source(),
        &strings(&["README.fixture.md", "app/src/lib.rs", "wire/src/lib.rs"]),
    )
    .expect("known mixed-class inputs remain local");
    assert_eq!(report.mode(), ChangeSelectionMode::Local);
    assert_ne!(report.gate_ids(), GateId::BATTERY);
    let encoded = report.encode().expect("encode canonical local report");
    assert_eq!(
        ChangeSelectionReport::decode(&encoded).expect("decode canonical local report"),
        report,
    );
    let encoded = String::from_utf8(encoded).expect("selection report is UTF-8");
    let exhaustive = encoded.replace("\"mode\":\"local\"", "\"mode\":\"exhaustive\"");
    assert!(ChangeSelectionReport::decode(exhaustive.as_bytes()).is_err());
}

#[test]
fn every_selected_family_publishes_exact_object_reach() {
    let changed = strings(&[
        "README.fixture.md",
        "tools/reach_fixture.py",
        "wire/src/lib.rs",
    ]);
    let report = select_current_source_change_gates(source(), &changed)
        .expect("mixed source classes retain object reach");

    assert_eq!(report.reach().len(), report.gate_ids().len());
    for gate_id in report.gate_ids() {
        let reach = report
            .reach_for(*gate_id)
            .expect("every selected arm has one reach record");
        assert_eq!(reach.gate_id(), *gate_id);
        assert_eq!(reach.mode(), &ChangeReachMode::Focused);
        assert!(!reach.objects().is_empty());
        assert!(!reach.provenance().is_empty());
    }
    assert_always_on_reach(&report, &changed);
    assert_path_reach(&report, GateId::Formatters, "wire/src/lib.rs");
    assert_path_reach(&report, GateId::Complexity, "wire/src/lib.rs");
    assert_path_reach(&report, GateId::Reachability, "tools/reach_fixture.py");
}

fn assert_always_on_reach(report: &ChangeSelectionReport, changed: &[String]) {
    for gate_id in ALWAYS {
        let reach = report.reach_for(gate_id).expect("always-on reach exists");
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
        assert!(reach.provenance().iter().any(|edge| {
            edge.object() == &ChangeReachObject::Repository
                && edge.origin() == &ChangeReachOrigin::Always
        }));
        for source in changed {
            assert!(reach.provenance().iter().any(|edge| {
                matches!(
                    edge.origin(),
                    ChangeReachOrigin::Changed { source: observed } if observed == source
                )
            }));
        }
    }
}

fn assert_path_reach(report: &ChangeSelectionReport, gate_id: GateId, path: &str) {
    let reach = report.reach_for(gate_id).expect("path arm reach exists");
    let object = ChangeReachObject::Path {
        path: path.to_owned(),
    };
    assert!(reach.objects().contains(&object));
    assert!(reach.provenance().iter().any(|edge| {
        edge.object() == &object
            && matches!(
                edge.origin(),
                ChangeReachOrigin::Changed { source } if source == path
            )
    }));
}

#[test]
fn dependency_expansion_revalidates_path_package_target_and_check_reach() {
    let tracked = strings(&["Cargo.toml", "wire/Cargo.toml", "wire/src/lib.rs"]);
    let policy = policy(&tracked, false, None, &[], &[], &[]);
    let report = selection(&["wire/src/lib.rs"], &policy);
    let source = "wire/src/lib.rs";
    let path = ChangeReachObject::Path {
        path: source.to_owned(),
    };
    let package = ChangeReachObject::Package {
        package: "wire".to_owned(),
    };
    let target = ChangeReachObject::Target {
        package: "wire".to_owned(),
        target: "lib:wire".to_owned(),
    };
    let check = ChangeReachObject::Check {
        check: "cargo-check".to_owned(),
    };
    let objects = vec![path.clone(), package.clone(), target.clone(), check.clone()];
    let provenance = vec![
        ChangeReachEdge::changed(source, path.clone()),
        ChangeReachEdge::dependency(source, path, package.clone()),
        ChangeReachEdge::dependency(source, package, target.clone()),
        ChangeReachEdge::dependency(source, target, check),
    ];
    let reach = report
        .reach()
        .iter()
        .map(|reach| {
            if reach.gate_id() == GateId::RustTypecheck {
                ChangeArmReach::exhaustive(
                    reach.gate_id(),
                    ChangeReachAuthority::Workspace,
                    "workspace build policy changed",
                    objects.clone(),
                    provenance.clone(),
                )
            } else {
                reach.clone()
            }
        })
        .collect();

    let expanded = report
        .with_reach(reach)
        .expect("dependency reach validates");
    let rust = expanded
        .reach_for(GateId::RustTypecheck)
        .expect("Rust reach remains selected");
    assert_eq!(
        rust.mode(),
        &ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::Workspace,
            reason: "workspace build policy changed".to_owned(),
        }
    );
    assert_eq!(rust.objects(), objects);
    let encoded = expanded.encode().expect("expanded reach encodes");
    assert_eq!(
        ChangeSelectionReport::decode(&encoded).expect("expanded reach decodes"),
        expanded,
    );
}

#[test]
fn dependency_reach_refuses_cross_source_provenance_and_rooted_cycles() {
    let tracked = strings(&[
        "Cargo.toml",
        "gate/Cargo.toml",
        "gate/src/lib.rs",
        "wire/Cargo.toml",
        "wire/src/lib.rs",
    ]);
    let policy = policy(&tracked, false, None, &[], &[], &[]);
    let report = selection(&["gate/src/lib.rs", "wire/src/lib.rs"], &policy);
    let gate_path = ChangeReachObject::Path {
        path: "gate/src/lib.rs".to_owned(),
    };
    let wire_path = ChangeReachObject::Path {
        path: "wire/src/lib.rs".to_owned(),
    };
    let package = ChangeReachObject::Package {
        package: "wire".to_owned(),
    };
    let target = ChangeReachObject::Target {
        package: "wire".to_owned(),
        target: "lib:wire".to_owned(),
    };

    let mut cross_source_provenance = vec![
        ChangeReachEdge::changed("gate/src/lib.rs", gate_path.clone()),
        ChangeReachEdge::changed("wire/src/lib.rs", wire_path.clone()),
        ChangeReachEdge::dependency("gate/src/lib.rs", wire_path.clone(), package.clone()),
    ];
    cross_source_provenance.sort();
    let error = report
        .clone()
        .with_reach(replace_arm_reach(
            &report,
            GateId::RustTypecheck,
            vec![gate_path.clone(), wire_path.clone(), package.clone()],
            cross_source_provenance,
        ))
        .expect_err("dependency provenance cannot cross changed-source roots");
    assert!(
        error.detail().contains("unproven dependency source"),
        "{error}"
    );

    let mut cyclic_provenance = vec![
        ChangeReachEdge::changed("gate/src/lib.rs", gate_path.clone()),
        ChangeReachEdge::changed("wire/src/lib.rs", wire_path.clone()),
        ChangeReachEdge::dependency("wire/src/lib.rs", wire_path.clone(), package.clone()),
        ChangeReachEdge::dependency("wire/src/lib.rs", package.clone(), target.clone()),
        ChangeReachEdge::dependency("wire/src/lib.rs", target.clone(), package.clone()),
    ];
    cyclic_provenance.sort();
    let error = report
        .clone()
        .with_reach(replace_arm_reach(
            &report,
            GateId::RustTypecheck,
            vec![gate_path, wire_path, package, target],
            cyclic_provenance,
        ))
        .expect_err("rooted dependency cycles cannot enter canonical reach");
    assert!(error.detail().contains("dependency cycle"), "{error}");
}

fn replace_arm_reach(
    report: &ChangeSelectionReport,
    gate_id: GateId,
    mut objects: Vec<ChangeReachObject>,
    provenance: Vec<ChangeReachEdge>,
) -> Vec<ChangeArmReach> {
    objects.sort();
    report
        .reach()
        .iter()
        .map(|reach| {
            if reach.gate_id() == gate_id {
                ChangeArmReach::focused(gate_id, objects.clone(), provenance.clone())
            } else {
                reach.clone()
            }
        })
        .collect()
}
