use super::*;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
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
        let status = Command::new("/usr/bin/git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&root)
            .status()
            .expect("launch fixture Git");
        assert!(status.success());
        Self { root }
    }

    fn stage(&self, path: &str, bytes: &[u8]) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture path parent");
        }
        fs::write(target, bytes).expect("write fixture path");
        let status = Command::new("/usr/bin/git")
            .args(["add", "--", path])
            .current_dir(&self.root)
            .status()
            .expect("launch fixture Git add");
        assert!(status.success());
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
    for authority in ["Cargo.lock", "rust-toolchain.toml"] {
        let error = select_change_gates(source(), &strings(&[authority]), &policy)
            .expect_err("repository build authorities must refuse");
        assert_eq!(
            error.detail(),
            format!("{authority:?} changes repository build or test selection authority")
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
fn policy_and_unknown_changes_refuse_without_manufacturing_applicable_execution() {
    let tracked = strings(&["assets/opaque.bin", "notes/readme.md", "spice-rust.toml"]);
    let suffixes = strings(&[".md"]);
    let policy = policy(&tracked, true, Some(&suffixes), &[], &[], &[]);
    let authority = select_change_gates(source(), &strings(&["spice-rust.toml"]), &policy)
        .expect_err("packaged selector authority must refuse");
    assert_eq!(
        authority.detail(),
        "\"spice-rust.toml\" changes packaged selection policy"
    );
    let unknown = select_change_gates(source(), &strings(&["assets/opaque.bin"]), &policy)
        .expect_err("unclassified packaged path must refuse");
    assert_eq!(
        unknown.detail(),
        "\"assets/opaque.bin\" has no repository-declared packaged policy class"
    );
}

#[test]
fn generic_selection_has_no_spice_frontend_or_workspace_topology() {
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
    assert!(!report.gate_ids().contains(&GateId::CanvasWebTypecheck));
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
fn current_source_override_retains_the_richer_workspace_selector() {
    let report = select_current_source_change_gates(
        source(),
        &strings(&["app/browser/canvas-coordinator.mjs", "wire/src/lib.rs"]),
    )
    .expect("select current source changes");

    contains(
        &report,
        &[
            GateId::CanvasWebTypecheck,
            GateId::JavascriptUnused,
            GateId::Formatters,
        ],
    );
    assert_eq!(
        report.gate_ids(),
        BTreeSet::from([
            GateId::MergeIntegrity,
            GateId::PlanPhase,
            GateId::Staging,
            GateId::LocalPaths,
            GateId::FileShape,
            GateId::CanvasWebTypecheck,
            GateId::JavascriptUnused,
            GateId::EnvPolicy,
            GateId::EnvNameLedger,
            GateId::Complexity,
            GateId::MagicNumbers,
            GateId::Formatters,
            GateId::MarkdownLinks,
        ])
        .into_iter()
        .collect::<Vec<_>>(),
    );
    assert_eq!(report.applicable_gate_ids(), GateId::BATTERY);
    assert_ne!(report.gate_ids(), report.applicable_gate_ids());
}

#[test]
fn current_source_classification_gaps_refuse_with_exact_paths() {
    for (path, condition) in [
        (
            "assets/opaque.bin",
            "has no compiled change-local policy class",
        ),
        (
            "tools/outside.rs",
            "is Rust source outside every compiled workspace root",
        ),
        (
            "gate/tools/outside.rs",
            "is Rust source outside every compiled workspace root",
        ),
        (
            "tools/component/Cargo.toml",
            "is a package manifest outside every compiled workspace root",
        ),
    ] {
        let error = select_current_source_change_gates(source(), &strings(&[path]))
            .expect_err("classification gap must refuse");
        assert_eq!(error.detail(), format!("{path:?} {condition}"));
    }
}

#[test]
fn current_source_authorities_refuse_and_local_reports_are_canonical() {
    for path in [
        "spice-rust.toml",
        "gate/src/change_selection.rs",
        "gate/src/change_selection/policy.rs",
        "spice/src/change_gate.rs",
        "xtask/src/gate_command.rs",
    ] {
        let error = select_current_source_change_gates(source(), &strings(&[path]))
            .expect_err("selector authority must refuse");
        assert_eq!(
            error.detail(),
            format!("{path:?} changes gate or toolchain selection authority")
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
fn current_source_report_modules_remain_selector_authorities() {
    let error = select_current_source_change_gates(
        source(),
        &strings(&["gate/src/test_evidence/reports.rs"]),
    )
    .expect_err("moved report authority must refuse");

    assert_eq!(
        error.detail(),
        "\"gate/src/test_evidence/reports.rs\" changes gate or toolchain selection authority"
    );
}

#[test]
fn every_changed_path_and_always_on_arm_retains_provenance() {
    let tracked = strings(&["README.md", "src/lib.rs"]);
    let policy = policy(&tracked, false, None, &[], &[], &[]);
    let report = selection(&["README.md", "src/lib.rs"], &policy);
    for gate_id in ALWAYS {
        assert!(report.gate_ids().contains(&gate_id));
    }
    for path in report.changed_sources() {
        assert!(report.edges().iter().any(|edge| edge.source() == path));
    }
}
