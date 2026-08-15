use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn syntax_keys_ignore_trivia_but_exact_arms_do_not() {
    let fixture = Fixture::new("trivia");
    fixture.write("gate/src/value.rs", "fn value() -> u64 { 7 }\n");
    let first = fixture.closure(&["gate/src/value.rs"]);
    fixture.write(
        "gate/src/value.rs",
        "// retained comment\nfn value() -> u64 {    7 }\n",
    );
    let second = fixture.closure(&["gate/src/value.rs"]);

    assert_eq!(
        first.key(GateId::RustTypecheck),
        second.key(GateId::RustTypecheck)
    );
    assert_ne!(
        first.key(GateId::Formatters),
        second.key(GateId::Formatters)
    );
    let comparison = first.compare(&second).expect("known syntax compares");
    assert!(comparison.reusable().contains(&GateId::RustTypecheck));
    assert!(comparison.invalidated().contains(&GateId::Formatters));
}

#[test]
fn identical_semantic_closures_reuse_across_distinct_commit_tree_identities() {
    let fixture = Fixture::new("cross-commit");
    fixture.write("gate/src/value.rs", "pub fn value() -> u64 { 7 }\n");
    let paths = ["gate/src/value.rs"];
    let first = fixture.closure_at(&paths, b"commit one selected tree");
    let second = fixture.closure_at(&paths, b"commit two selected tree");

    assert_ne!(first.source(), second.source());
    let comparison = first
        .compare(&second)
        .expect("cross-commit closure compares");
    assert_eq!(
        comparison.reusable().len(),
        GateId::BATTERY.len() - 3,
        "only the three always-executed arms are deliberately invalidated",
    );
    assert!(comparison.changed_sources().is_empty());
}

#[test]
fn semantic_changes_rename_moves_and_selector_authority_invalidate_owned_arms() {
    let fixture = Fixture::new("movement");
    fixture.write("gate/src/alpha.rs", "fn value() -> u64 { 7 }\n");
    fixture.write("gate/src/change_selection.rs", "pub fn selector() {}\n");
    let first = fixture.closure(&["gate/src/alpha.rs", "gate/src/change_selection.rs"]);
    fixture.write("gate/src/beta.rs", "fn value() -> u64 { 7 }\n");
    fs::remove_file(fixture.root.join("gate/src/alpha.rs")).expect("remove renamed source");
    let second = fixture.closure(&["gate/src/beta.rs", "gate/src/change_selection.rs"]);
    let comparison = first.compare(&second).expect("known rename compares");
    assert!(comparison.invalidated().contains(&GateId::RustTypecheck));
    assert_eq!(
        comparison.changed_sources(),
        ["gate/src/alpha.rs", "gate/src/beta.rs"]
    );

    fixture.write(
        "gate/src/change_selection.rs",
        "pub fn changed_selector() {}\n",
    );
    let authority = fixture.closure(&["gate/src/beta.rs", "gate/src/change_selection.rs"]);
    assert_eq!(
        authority
            .sources()
            .iter()
            .find(|source| source.path() == "gate/src/change_selection.rs")
            .map(GateClosureSource::class),
        Some(GateSourceClass::SelectorAuthority),
    );
    assert!(
        authority
            .arms()
            .iter()
            .all(|arm| first.key(arm.gate_id()) != Some(arm.key()))
    );
    let comparison = second
        .compare(&authority)
        .expect("selector-authority movement compares");
    assert!(comparison.reusable().is_empty());
    assert!(
        comparison
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("selector authority")),
    );
}

#[test]
fn generated_boundaries_do_not_enter_language_arms_and_unknown_movement_refuses() {
    let fixture = Fixture::new("boundaries");
    fixture.write("generated/out.rs", "fn generated() {}\n");
    fixture.write("assets/opaque.bin", "opaque\n");
    let first = GateSemanticClosure::capture(
        fixture.source(),
        &fixture.root,
        &[
            "assets/opaque.bin".to_owned(),
            "generated/out.rs".to_owned(),
        ],
        &[],
        &["generated".to_owned()],
    )
    .expect("capture boundary closure");
    let generated = first
        .sources()
        .iter()
        .find(|source| source.path() == "generated/out.rs")
        .expect("generated source retained");
    assert_eq!(generated.class(), GateSourceClass::Generated);
    assert!(!generated.gates().contains(&GateId::RustTypecheck));

    fixture.write("generated/out.rs", "fn generated() { unreachable!() }\n");
    let generated_change = GateSemanticClosure::capture(
        fixture.source(),
        &fixture.root,
        &[
            "assets/opaque.bin".to_owned(),
            "generated/out.rs".to_owned(),
        ],
        &["generated/out.rs".to_owned()],
        &["generated".to_owned()],
    )
    .expect("capture generated change");
    assert_eq!(
        first.key(GateId::RustTypecheck),
        generated_change.key(GateId::RustTypecheck),
    );
    assert_ne!(
        first.key(GateId::LocalPaths),
        generated_change.key(GateId::LocalPaths),
    );

    fixture.write("assets/opaque.bin", "changed\n");
    let second = GateSemanticClosure::capture(
        fixture.source(),
        &fixture.root,
        &[
            "assets/opaque.bin".to_owned(),
            "generated/out.rs".to_owned(),
        ],
        &[],
        &["generated".to_owned()],
    )
    .expect("capture moved opaque closure");
    let comparison = first.compare(&second).expect("opaque movement compares");
    assert!(comparison.reusable().is_empty());
    assert!(
        comparison
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("assets/opaque.bin")),
        "{:?}",
        comparison.diagnostics(),
    );
}

#[test]
fn selected_change_inventory_only_invalidates_selected_change_arms() {
    let fixture = Fixture::new("changed-inventory");
    fixture.write("gate/src/lib.rs", "pub fn fixture() {}\n");
    fixture.write("README.md", "stable\n");
    let paths = ["README.md", "gate/src/lib.rs"];
    let first = fixture.closure_with_changes(&paths, &["gate/src/lib.rs"]);
    let second = fixture.closure_with_changes(&paths, &["README.md"]);

    assert_ne!(
        first.key(GateId::LocalPaths),
        second.key(GateId::LocalPaths)
    );
    assert_eq!(
        first.key(GateId::RustTypecheck),
        second.key(GateId::RustTypecheck),
    );
}

#[test]
fn compiler_configuration_and_complete_workspace_inputs_reach_their_owners() {
    let fixture = Fixture::new("compiler-inputs");
    fixture.write("artifact/src/lib.rs", "pub fn artifact() -> u64 { 1 }\n");
    fixture.write(
        "pyproject.toml",
        "[tool.pyright]\ntypeCheckingMode = \"strict\"\n",
    );
    let paths = ["artifact/src/lib.rs", "pyproject.toml"];
    let first = fixture.closure(&paths);

    fixture.write("artifact/src/lib.rs", "pub fn artifact() -> u64 { 2 }\n");
    let rust_change = fixture.closure(&paths);
    assert_ne!(
        first.key(GateId::CanvasWebTypecheck),
        rust_change.key(GateId::CanvasWebTypecheck),
    );

    fixture.write(
        "pyproject.toml",
        "[tool.pyright]\ntypeCheckingMode = \"basic\"\n",
    );
    let configuration_change = fixture.closure(&paths);
    assert_ne!(
        rust_change.key(GateId::PythonTypecheck),
        configuration_change.key(GateId::PythonTypecheck),
    );
}

#[test]
fn gate_implementation_sources_reach_their_compiled_arm_families() {
    let fixture = Fixture::new("gate-implementation");
    fixture.write(
        "gate/src/toolchain/analyze/javascript.rs",
        "pub fn analyze() -> u64 { 1 }\n",
    );
    fixture.write(
        "gate/src/metrics/measure.rs",
        "pub fn measure() -> u64 { 1 }\n",
    );
    let paths = [
        "gate/src/metrics/measure.rs",
        "gate/src/toolchain/analyze/javascript.rs",
    ];
    let first = fixture.closure(&paths);

    fixture.write(
        "gate/src/toolchain/analyze/javascript.rs",
        "pub fn analyze() -> u64 { 2 }\n",
    );
    let toolchain_change = fixture.closure(&paths);
    assert_ne!(
        first.key(GateId::JavascriptUnused),
        toolchain_change.key(GateId::JavascriptUnused),
    );
    assert_eq!(
        first.key(GateId::RepoDocs),
        toolchain_change.key(GateId::RepoDocs),
    );

    fixture.write(
        "gate/src/metrics/measure.rs",
        "pub fn measure() -> u64 { 2 }\n",
    );
    let metrics_change = fixture.closure(&paths);
    assert_ne!(
        toolchain_change.key(GateId::FileShape),
        metrics_change.key(GateId::FileShape),
    );
}

#[test]
fn static_dependency_reach_is_transitive_and_dynamic_edges_fail_closed() {
    let fixture = Fixture::new("dependencies");
    fixture.write(
        "gate/src/dependency_fixture.rs",
        "pub const MESSAGE: &str = include_str!(\"message.txt\");\n",
    );
    fixture.write("gate/src/message.txt", "first\n");
    let paths = ["gate/src/dependency_fixture.rs", "gate/src/message.txt"];
    let first = fixture.closure(&paths);
    let source = first
        .sources()
        .iter()
        .find(|source| source.path() == "gate/src/dependency_fixture.rs")
        .expect("Rust source closure");
    assert_eq!(source.dependencies().len(), 1);
    assert_eq!(source.dependencies()[0].path(), "gate/src/message.txt");
    assert_eq!(source.dependencies()[0].mode(), GateDependencyMode::Exact);

    fixture.write("gate/src/message.txt", "second\n");
    let second = fixture.closure(&paths);
    assert_ne!(
        first.key(GateId::RustTypecheck),
        second.key(GateId::RustTypecheck),
    );
    assert_eq!(
        first.key(GateId::Formatters),
        second.key(GateId::Formatters)
    );

    fixture.write(
        "gate/src/dependency_fixture.rs",
        "pub const MESSAGE: &str = include_str!(concat!(\"message\", \".txt\"));\n",
    );
    let dynamic = fixture.closure(&paths);
    let source = dynamic
        .sources()
        .iter()
        .find(|source| source.path() == "gate/src/dependency_fixture.rs")
        .expect("dynamic Rust source closure");
    assert_eq!(source.class(), GateSourceClass::DynamicEdge);
    let comparison = second.compare(&dynamic).expect("dynamic closure compares");
    assert!(comparison.invalidated().contains(&GateId::RustTypecheck));
    assert!(
        comparison
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("dynamic Rust include_str!")),
        "{:?}",
        comparison.diagnostics(),
    );
    let stable_dynamic = dynamic
        .compare(&dynamic)
        .expect("stable dynamic closure compares");
    assert!(
        stable_dynamic
            .invalidated()
            .contains(&GateId::RustTypecheck),
    );
    assert!(stable_dynamic.reusable().contains(&GateId::Complexity));
}

#[test]
fn cargo_dynamic_compiler_targets_refuse_compilers_without_contaminating_independent_arms() {
    let fixture = Fixture::new("cargo-dynamic-inputs");
    fixture.write(
        "fold/Cargo.toml",
        "[package]\nname = \"fold\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    );
    fixture.write(
        "fold/build.rs",
        "fn main() { let _ = std::fs::read(\"schema.json\"); }\n",
    );
    fixture.write("fold/schema.json", "{\"version\":1}\n");
    fixture.write(
        "artifact/Cargo.toml",
        concat!(
            "[package]\nname = \"artifact\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
            "[lib]\nproc-macro = true\n",
        ),
    );
    fixture.write("artifact/src/lib.rs", "pub fn expand() {}\n");
    fixture.write("README.md", "first\n");
    let paths = [
        "README.md",
        "artifact/Cargo.toml",
        "artifact/src/lib.rs",
        "fold/Cargo.toml",
        "fold/build.rs",
        "fold/schema.json",
    ];
    let first = fixture.closure(&paths);

    for (path, kind) in [
        ("fold/build.rs", "build script"),
        ("artifact/src/lib.rs", "procedural-macro"),
    ] {
        let source = first
            .sources()
            .iter()
            .find(|source| source.path() == path)
            .expect("dynamic compiler boundary");
        assert_eq!(source.class(), GateSourceClass::DynamicEdge);
        assert!(source.gates().contains(&GateId::RustTypecheck));
        assert!(source.gates().contains(&GateId::CanvasWebTypecheck));
        assert!(
            source
                .diagnostic()
                .is_some_and(|detail| detail.contains(kind))
        );
    }

    fixture.write("fold/schema.json", "{\"version\":2}\n");
    fixture.write("README.md", "second\n");
    let second = fixture.closure(&paths);
    let comparison = first
        .compare(&second)
        .expect("dynamic Cargo inputs compare");
    assert!(comparison.invalidated().contains(&GateId::RustTypecheck));
    assert!(
        comparison
            .invalidated()
            .contains(&GateId::CanvasWebTypecheck),
    );
    assert!(comparison.reusable().contains(&GateId::Complexity));
    assert!(comparison.reusable().contains(&GateId::MagicNumbers));
    assert!(
        comparison
            .diagnostics()
            .iter()
            .any(|detail| { detail.contains("Cargo/rustc-discovered dependency evidence") })
    );
}

#[test]
fn disabled_cargo_build_target_does_not_create_a_dynamic_boundary() {
    let fixture = Fixture::new("disabled-build-target");
    fixture.write(
        "fold/Cargo.toml",
        concat!(
            "[package]\nname = \"fold\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
            "build = false\n",
        ),
    );
    fixture.write("fold/build.rs", "fn main() {}\n");
    let closure = fixture.closure(&["fold/Cargo.toml", "fold/build.rs"]);
    let build = closure
        .sources()
        .iter()
        .find(|source| source.path() == "fold/build.rs")
        .expect("disabled build target source");

    assert_eq!(build.class(), GateSourceClass::Known);
    assert!(
        closure
            .compare(&closure)
            .expect("disabled build target compares")
            .reusable()
            .contains(&GateId::RustTypecheck),
    );
}

#[test]
fn syntax_dependency_reach_carries_transitive_exact_generated_inputs() {
    let fixture = Fixture::new("transitive-dependencies");
    fixture.write("gate/src/root.rs", "include!(\"generated/middle.rs\");\n");
    fixture.write(
        "gate/src/generated/middle.rs",
        "pub const MESSAGE: &str = include_str!(\"leaf.txt\");\n",
    );
    fixture.write("gate/src/generated/leaf.txt", "first\n");
    let paths = [
        "gate/src/generated/leaf.txt".to_owned(),
        "gate/src/generated/middle.rs".to_owned(),
        "gate/src/root.rs".to_owned(),
    ];
    let generated = ["gate/src/generated".to_owned()];
    let first =
        GateSemanticClosure::capture(fixture.source(), &fixture.root, &paths, &[], &generated)
            .expect("capture transitive dependency closure");

    fixture.write("gate/src/generated/leaf.txt", "second\n");
    let second = GateSemanticClosure::capture(
        fixture.source(),
        &fixture.root,
        &paths,
        &["gate/src/generated/leaf.txt".to_owned()],
        &generated,
    )
    .expect("capture changed transitive dependency closure");

    assert_ne!(
        first.key(GateId::RustTypecheck),
        second.key(GateId::RustTypecheck),
        "the exact leaf reached through a syntax dependency must invalidate Rust",
    );
    assert_eq!(
        first.key(GateId::Complexity),
        second.key(GateId::Complexity),
        "the generated leaf is outside unrelated nondependency arms",
    );
}

#[test]
fn rust_module_names_containing_path_are_resolved_without_false_dynamic_edges() {
    let fixture = Fixture::new("rust-path-module-name");
    fixture.write("artifact/src/mod.rs", "mod pathway;\n");
    fixture.write("artifact/src/pathway.rs", "pub fn reached() {}\n");
    let closure = fixture.closure(&["artifact/src/mod.rs", "artifact/src/pathway.rs"]);
    let source = closure
        .sources()
        .iter()
        .find(|source| source.path() == "artifact/src/mod.rs")
        .expect("Rust module source closure");

    assert_eq!(source.class(), GateSourceClass::Known);
    assert_eq!(source.dependencies().len(), 1);
    assert_eq!(source.dependencies()[0].path(), "artifact/src/pathway.rs");
}

#[test]
fn exact_dependency_mode_preserves_embedded_rust_trivia() {
    let fixture = Fixture::new("exact-dependency");
    fixture.write(
        "gate/src/dependency_fixture.rs",
        "pub const EMBEDDED: &str = include_str!(\"embedded.rs\");\n",
    );
    fixture.write("gate/src/embedded.rs", "pub const VALUE: u64 = 7;\n");
    let paths = ["gate/src/dependency_fixture.rs", "gate/src/embedded.rs"];
    let first = fixture.closure(&paths);

    fixture.write(
        "gate/src/embedded.rs",
        "// embedded bytes changed\npub const VALUE: u64 = 7;\n",
    );
    let second = fixture.closure(&paths);

    assert_ne!(
        first.key(GateId::RustTypecheck),
        second.key(GateId::RustTypecheck),
        "include_str! consumes exact bytes even when its target parses as Rust",
    );
}

#[test]
fn parser_gaps_emit_explicit_affected_arm_refusals() {
    let fixture = Fixture::new("parser-gap");
    fixture.write("gate/src/broken.rs", "pub fn broken(\n");
    let closure = fixture.closure(&["gate/src/broken.rs"]);
    let source = closure
        .sources()
        .iter()
        .find(|source| source.path() == "gate/src/broken.rs")
        .expect("broken source closure");
    assert_eq!(source.class(), GateSourceClass::ParserGap);
    let comparison = closure
        .compare(&closure)
        .expect("parser-gap closure compares");
    assert!(comparison.invalidated().contains(&GateId::RustTypecheck));
    assert!(comparison.invalidated().contains(&GateId::Complexity));
    assert!(
        comparison
            .diagnostics()
            .iter()
            .any(|detail| detail.contains("Tree-sitter")),
    );
}

#[test]
fn canonical_wire_recomputes_every_arm_key() {
    let fixture = Fixture::new("wire");
    fixture.write("gate/src/lib.rs", "pub fn fixture() {}\n");
    let closure = fixture.closure(&["gate/src/lib.rs"]);
    let bytes = closure.encode().expect("encode closure");
    assert_eq!(
        GateSemanticClosure::decode(&bytes).expect("decode closure"),
        closure
    );

    let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("closure value");
    value["arms"][0]["key"] =
        serde_json::json!(Digest::compute(Domain::GateReceipt, b"forged semantic key").to_string());
    let forged = serde_json::to_vec(&value).expect("encode forged closure");
    assert!(GateSemanticClosure::decode(&forged).is_err());

    assert!(
        GateSemanticClosure::capture(
            fixture.source(),
            &fixture.root,
            &["gate/src/lib.rs".to_owned()],
            &[],
            &["../generated".to_owned()],
        )
        .is_err(),
        "malformed generated selectors fail closed before matching a path",
    );
    assert!(
        GateSemanticClosure::capture(
            fixture.source(),
            &fixture.root,
            &["../gate/src/lib.rs".to_owned()],
            &[],
            &[],
        )
        .is_err(),
        "noncanonical selected paths cannot escape the captured tree",
    );
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-semantic-closure-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create semantic fixture");
        Self { root }
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(path, text).expect("write semantic fixture");
    }

    fn source(&self) -> Digest {
        Digest::compute(Domain::GateTree, self.root.as_os_str().as_encoded_bytes())
    }

    fn closure(&self, paths: &[&str]) -> GateSemanticClosure {
        self.closure_with_changes(paths, &[])
    }

    fn closure_at(&self, paths: &[&str], source: &[u8]) -> GateSemanticClosure {
        let mut paths = paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();
        paths.sort();
        GateSemanticClosure::capture(
            Digest::compute(Domain::GateTree, source),
            &self.root,
            &paths,
            &[],
            &[],
        )
        .expect("capture semantic fixture at source")
    }

    fn closure_with_changes(&self, paths: &[&str], changes: &[&str]) -> GateSemanticClosure {
        let mut paths = paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();
        paths.sort();
        let changes = changes
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();
        GateSemanticClosure::capture(self.source(), &self.root, &paths, &changes, &[])
            .expect("capture semantic fixture")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove semantic fixture");
    }
}
