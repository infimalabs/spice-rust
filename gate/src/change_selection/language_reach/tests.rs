use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::*;
use crate::{ChangeReachMode, select_current_source_change_gates};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn javascript_reach_keeps_import_and_symbol_dependents_but_not_a_peer_component() {
    let fixture = Fixture::new("javascript-component");
    fixture.write(
        "package.json",
        "{\"name\":\"fixture\",\"dependencies\":{\"external\":\"1.0.0\"}}\n",
    );
    fixture.write(
        "src/entry.js",
        concat!(
            "import assert from 'node:assert';\n",
            "import external from 'external';\n",
            "import { used } from './dep.js';\n",
            "assert(external);\n",
            "console.log(used);\n",
        ),
    );
    fixture.write("src/dep.js", "export const used = 1;\n");
    fixture.write("tests/consumer.js", "console.log(used);\n");
    fixture.write("other/isolated.js", "const lonely = 1;\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/entry.js".to_owned()])
        .expect("JavaScript change selects its arms");

    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &["src".to_owned()],
        &["tests".to_owned()],
        selection,
    )
    .expect("JavaScript graph compiles");
    assert_eq!(
        reached_paths(&expanded, GateId::JavascriptUnused),
        BTreeSet::from([
            "src/dep.js".to_owned(),
            "src/entry.js".to_owned(),
            "tests/consumer.js".to_owned(),
        ])
    );
    assert_eq!(
        expanded
            .reach_for(GateId::EnvNameLedger)
            .expect("ledger selected")
            .mode(),
        &ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason:
                "the environment name ledger compares one global declaration/reference inventory"
                    .to_owned(),
        }
    );
}

#[test]
fn javascript_multi_root_canvas_component_produces_one_acyclic_canonical_reach() {
    let fixture = Fixture::new("javascript-canvas-multi-root");
    fixture.write(
        "app/browser/canvas-fixture.mjs",
        "export const configureFixture = () => {};\n",
    );
    fixture.write(
        "app/browser/canvas.spec.mjs",
        concat!(
            "import { configureFixture } from './canvas-fixture.mjs';\n",
            "configureFixture();\n",
        ),
    );
    fixture.write(
        "app/browser/canvas-coordinator.mjs",
        concat!(
            "import { configureFixture } from './canvas-fixture.mjs';\n",
            "configureFixture();\n",
        ),
    );
    fixture.write("app/browser/unrelated.mjs", "export const unrelated = 1;\n");
    let tracked = fixture.paths();
    let changes = [
        "app/browser/canvas-fixture.mjs".to_owned(),
        "app/browser/canvas.spec.mjs".to_owned(),
    ];
    let selection = select_current_source_change_gates(source(), &changes)
        .expect("the two changed Canvas sources select JavaScript analysis");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("one connected multi-root JavaScript component stays acyclic");
    assert_eq!(
        reached_paths(&expanded, GateId::JavascriptUnused),
        BTreeSet::from([
            "app/browser/canvas-coordinator.mjs".to_owned(),
            "app/browser/canvas-fixture.mjs".to_owned(),
            "app/browser/canvas.spec.mjs".to_owned(),
        ])
    );
    let reach = expanded
        .reach_for(GateId::JavascriptUnused)
        .expect("JavaScript arm remains selected");
    assert_eq!(reach.mode(), &ChangeReachMode::Focused);
    assert_eq!(
        reach.provenance().iter().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            ChangeReachEdge::changed(
                "app/browser/canvas-fixture.mjs",
                path_object("app/browser/canvas-fixture.mjs"),
            ),
            ChangeReachEdge::changed(
                "app/browser/canvas.spec.mjs",
                path_object("app/browser/canvas.spec.mjs"),
            ),
            ChangeReachEdge::dependency(
                "app/browser/canvas-fixture.mjs",
                path_object("app/browser/canvas-fixture.mjs"),
                path_object("app/browser/canvas-coordinator.mjs"),
            ),
        ])
    );
}

#[test]
fn csharp_reach_keeps_namespace_and_inheritance_dependents_only() {
    let fixture = Fixture::new("csharp-component");
    fixture.write(
        "src/Widget.cs",
        "namespace Product.Core;\nclass Widget { private int unused; }\n",
    );
    fixture.write(
        "src/Child.cs",
        "using Product.Core;\nclass Child : Widget { }\n",
    );
    fixture.write("other/Other.cs", "namespace Other;\nclass OtherType { }\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/Widget.cs".to_owned()])
        .expect("C# change selects its arms");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("C# graph compiles");
    assert_eq!(
        reached_paths(&expanded, GateId::CsharpUnusedCandidates),
        BTreeSet::from(["src/Child.cs".to_owned(), "src/Widget.cs".to_owned()])
    );
}

#[test]
fn explicit_python_package_directory_retains_identity_and_exact_component() {
    let fixture = Fixture::new("explicit-python-package");
    fixture.write("src/sample/__init__.py", "from . import a\n");
    fixture.write("src/sample/a.py", "from sample import b\nVALUE = b.VALUE\n");
    fixture.write("src/sample/b.py", "VALUE = 1\n");
    fixture.write(
        "src/sample/c.py",
        "import sample.b\nVALUE = sample.b.VALUE\n",
    );
    fixture.write("src/sample/d.py", "from .c import VALUE\n");
    fixture.write("src/other/__init__.py", "\n");
    fixture.write("src/other/alone.py", "VALUE = 2\n");
    fixture.write(
        "tests/test_sample.py",
        "from sample import a\nassert a.VALUE\n",
    );
    let tracked = fixture.paths();
    let changed = "src/sample/b.py";
    let selection = select_current_source_change_gates(source(), &[changed.to_owned()])
        .expect("explicit-package Python change selects its arms");

    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &["src/sample".to_owned(), "src/other".to_owned()],
        &["tests".to_owned()],
        selection,
    )
    .expect("explicit Python package graph compiles");
    let expected = BTreeSet::from([
        "src/sample/__init__.py".to_owned(),
        "src/sample/a.py".to_owned(),
        "src/sample/b.py".to_owned(),
        "src/sample/c.py".to_owned(),
        "src/sample/d.py".to_owned(),
        "tests/test_sample.py".to_owned(),
    ]);
    let expected_provenance = BTreeSet::from([
        ChangeReachEdge::changed(changed, path_object(changed)),
        ChangeReachEdge::dependency(
            changed,
            path_object(changed),
            path_object("src/sample/a.py"),
        ),
        ChangeReachEdge::dependency(
            changed,
            path_object(changed),
            path_object("src/sample/c.py"),
        ),
        ChangeReachEdge::dependency(
            changed,
            path_object("src/sample/a.py"),
            path_object("src/sample/__init__.py"),
        ),
        ChangeReachEdge::dependency(
            changed,
            path_object("src/sample/a.py"),
            path_object("tests/test_sample.py"),
        ),
        ChangeReachEdge::dependency(
            changed,
            path_object("src/sample/c.py"),
            path_object("src/sample/d.py"),
        ),
    ]);
    for gate in PYTHON_ARMS {
        assert_eq!(reached_paths(&expanded, *gate), expected, "{gate}");
        let provenance = expanded
            .reach_for(*gate)
            .expect("Python consumer retains reach")
            .provenance()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(provenance, expected_provenance, "{gate}");
    }
}

#[test]
fn repository_root_python_package_retains_its_identity() {
    let fixture = Fixture::new("root-python-package");
    fixture.write("sample/__init__.py", "from . import b\n");
    fixture.write("sample/a.py", "from sample import b\n");
    fixture.write("sample/b.py", "VALUE = 1\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["sample/b.py".to_owned()])
        .expect("root-package Python change selects its arms");
    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &["sample".to_owned()],
        &[],
        selection,
    )
    .expect("repository-root Python package graph compiles");
    assert_eq!(
        reached_paths(&expanded, GateId::PythonTypecheck),
        BTreeSet::from([
            "sample/__init__.py".to_owned(),
            "sample/a.py".to_owned(),
            "sample/b.py".to_owned(),
        ])
    );
}

#[test]
fn ambiguous_or_invalid_python_package_directories_explicitly_escalate() {
    assert_python_root_escalation(
        "duplicate-package-identity",
        &["src/sample", "lib/sample"],
        &["src/sample/a.py", "lib/sample/b.py"],
        "package identity \"sample\" is declared by both",
    );
    assert_python_root_escalation(
        "overlapping-package-directories",
        &["src/sample", "src/sample/sub"],
        &["src/sample/a.py", "src/sample/sub/b.py"],
        "package directories \"src/sample\" and \"src/sample/sub\" overlap",
    );
    assert_python_root_escalation(
        "invalid-package-identity",
        &["src/sample-pkg"],
        &["src/sample-pkg/a.py"],
        "has no supported package identity",
    );
    assert_python_root_escalation(
        "keyword-package-identity",
        &["src/class"],
        &["src/class/a.py"],
        "has no supported package identity",
    );
    assert_python_root_escalation(
        "non-concrete-package-directory",
        &["src/*/sample"],
        &["src/*/sample/a.py"],
        "is not a concrete repository path",
    );
}

#[test]
fn malformed_peer_source_escalates_before_narrowed_execution() {
    let fixture = Fixture::new("malformed-javascript");
    fixture.write("src/changed.js", "const changed = 1;\n");
    fixture.write("other/broken.js", "const = ;\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/changed.js".to_owned()])
        .expect("JavaScript change selects its arms");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("malformed graph is explicit evidence");
    let reach = expanded
        .reach_for(GateId::JavascriptUnused)
        .expect("JavaScript arm remains selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("other/broken.js")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn dynamic_javascript_import_explicitly_escalates_before_narrowed_execution() {
    let fixture = Fixture::new("dynamic-javascript");
    fixture.write(
        "src/changed.js",
        "const target = './dependency.js';\nimport(target);\n",
    );
    fixture.write("src/dependency.js", "export const VALUE = 1;\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/changed.js".to_owned()])
        .expect("JavaScript change selects its arms");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("dynamic import becomes explicit reach");
    let reach = expanded
        .reach_for(GateId::JavascriptUnused)
        .expect("JavaScript arm remains selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("dynamic or unsupported specifier")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn unresolved_local_python_import_explicitly_escalates() {
    let fixture = Fixture::new("unresolved-python");
    fixture.write("src/pkg/__init__.py", "\n");
    fixture.write("src/pkg/changed.py", "import pkg.missing\n");
    let tracked = fixture.paths();
    let selection =
        select_current_source_change_gates(source(), &["src/pkg/changed.py".to_owned()])
            .expect("Python change selects its arms");

    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &["src/pkg".to_owned()],
        &[],
        selection,
    )
    .expect("unresolved import becomes explicit reach");
    let reach = expanded
        .reach_for(GateId::PythonTypecheck)
        .expect("Python typecheck remains selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("pkg.missing") && reason.contains("does not resolve")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn unsupported_csharp_reference_directive_explicitly_escalates() {
    let fixture = Fixture::new("unsupported-csharp-reference");
    fixture.write("src/Changed.cs", "#r \"external.dll\"\nclass Changed { }\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/Changed.cs".to_owned()])
        .expect("C# change selects its arms");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("unsupported reference becomes explicit reach");
    let reach = expanded
        .reach_for(GateId::CsharpUnusedCandidates)
        .expect("C# arm remains selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("C#")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn python_configuration_change_explicitly_escalates_every_graph_arm() {
    let fixture = Fixture::new("python-configuration");
    fixture.write("pyproject.toml", "[project]\nname = 'fixture'\n");
    fixture.write("src/pkg/module.py", "VALUE = 1\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["pyproject.toml".to_owned()])
        .expect("Python configuration selects graph arms");

    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &["src/pkg".to_owned()],
        &["tests".to_owned()],
        selection,
    )
    .expect("Python configuration reach compiles");

    for gate in PYTHON_ARMS {
        let reach = expanded.reach_for(*gate).expect("graph arm selected");
        assert!(matches!(
            reach.mode(),
            ChangeReachMode::Exhaustive {
                authority: ChangeReachAuthority::LanguageGraph,
                reason,
            } if reason == "Python configuration or configured-root authority changed"
        ));
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }
    let repository_shape = expanded
        .reach_for(GateId::RepoShape)
        .expect("repository shape remains selected");
    assert!(matches!(
        repository_shape.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason
            == "repository shape compares complete configured package and test root topology"
    ));
    assert_eq!(repository_shape.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn pyright_extra_path_defines_package_topology_without_joining_peer_packages() {
    let fixture = Fixture::new("python-extra-path");
    fixture.write("pyproject.toml", "[tool.pyright]\nextraPaths = ['lib']\n");
    fixture.write("lib/pkg/__init__.py", "\n");
    fixture.write("lib/pkg/a.py", "from pkg import b\nVALUE = b.VALUE\n");
    fixture.write("lib/pkg/b.py", "VALUE = 1\n");
    fixture.write("lib/other/alone.py", "VALUE = 2\n");
    fixture.write("tests/test_pkg.py", "from pkg import a\nassert a.VALUE\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["lib/pkg/a.py".to_owned()])
        .expect("configured-root Python change selects graph arms");

    let expanded = compile_language_reach(
        &fixture.root,
        &tracked,
        &[],
        &["tests".to_owned()],
        selection,
    )
    .expect("configured Pyright root compiles");

    let reach = expanded
        .reach_for(GateId::PythonTypecheck)
        .expect("Python typecheck remains selected");
    assert_eq!(reach.mode(), &ChangeReachMode::Focused);
    assert_eq!(
        reached_paths(&expanded, GateId::PythonTypecheck),
        BTreeSet::from([
            "lib/pkg/__init__.py".to_owned(),
            "lib/pkg/a.py".to_owned(),
            "lib/pkg/b.py".to_owned(),
            "tests/test_pkg.py".to_owned(),
        ])
    );
}

#[test]
fn javascript_package_authority_change_explicitly_escalates() {
    let fixture = Fixture::new("javascript-configuration");
    fixture.write(
        "package.json",
        "{\"name\":\"fixture\",\"dependencies\":{\"external\":\"1.0.0\"}}\n",
    );
    fixture.write("src/module.js", "import external from 'external';\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["package.json".to_owned()])
        .expect("JavaScript package authority selects its graph arm");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("JavaScript package authority reach compiles");
    let reach = expanded
        .reach_for(GateId::JavascriptUnused)
        .expect("JavaScript graph arm selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("non-language source")
    ));
    assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
}

#[test]
fn inherited_pyright_configuration_escalates_before_narrowing() {
    let fixture = Fixture::new("python-extends");
    fixture.write("pyproject.toml", "[tool.pyright]\nextends = 'base.toml'\n");
    fixture.write("src/pkg/module.py", "VALUE = 1\n");
    let tracked = fixture.paths();
    let selection = select_current_source_change_gates(source(), &["src/pkg/module.py".to_owned()])
        .expect("Python source selects graph arms");

    let expanded = compile_language_reach(&fixture.root, &tracked, &[], &[], selection)
        .expect("inherited configuration becomes explicit reach");
    let reach = expanded
        .reach_for(GateId::PythonTypecheck)
        .expect("Python typecheck remains selected");
    assert!(matches!(
        reach.mode(),
        ChangeReachMode::Exhaustive {
            authority: ChangeReachAuthority::LanguageGraph,
            reason,
        } if reason.contains("inherited tool.pyright extends")
    ));
}

fn assert_python_root_escalation(
    name: &str,
    package_roots: &[&str],
    paths: &[&str],
    expected_reason: &str,
) {
    let fixture = Fixture::new(name);
    for path in paths {
        fixture.write(path, "VALUE = 1\n");
    }
    let tracked = fixture.paths();
    let changed = paths.first().expect("root fixture has one changed path");
    let selection = select_current_source_change_gates(source(), &[(*changed).to_owned()])
        .expect("invalid-root Python change selects graph consumers");
    let package_roots = package_roots
        .iter()
        .map(|root| (*root).to_owned())
        .collect::<Vec<_>>();
    let expanded = compile_language_reach(&fixture.root, &tracked, &package_roots, &[], selection)
        .expect("unsupported package topology becomes explicit reach");
    for gate in PYTHON_ARMS {
        let reach = expanded.reach_for(*gate).expect("Python consumer selected");
        assert!(
            matches!(
                reach.mode(),
                ChangeReachMode::Exhaustive {
                    authority: ChangeReachAuthority::LanguageGraph,
                    reason,
                } if reason.contains(expected_reason)
            ),
            "{gate} did not retain package-topology cause: {reach:?}"
        );
        assert_eq!(reach.objects(), [ChangeReachObject::Repository]);
    }
}

fn reached_paths(report: &ChangeSelectionReport, gate: GateId) -> BTreeSet<String> {
    report
        .reach_for(gate)
        .expect("selected arm has reach")
        .objects()
        .iter()
        .filter_map(|object| match object {
            ChangeReachObject::Path { path } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

fn source() -> Digest {
    Digest::compute(Domain::GateTree, b"language reach fixture")
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let next = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gate-language-reach-{}-{name}-{next}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create language reach fixture");
        Self { root }
    }

    fn write(&self, path: &str, source: &str) {
        let absolute = self.root.join(path);
        fs::create_dir_all(absolute.parent().expect("fixture file has parent"))
            .expect("create fixture parent");
        fs::write(absolute, source).expect("write language reach fixture");
    }

    fn paths(&self) -> Vec<String> {
        fn collect(root: &Path, directory: &Path, paths: &mut Vec<String>) {
            for entry in fs::read_dir(directory).expect("read fixture directory") {
                let entry = entry.expect("read fixture entry");
                let path = entry.path();
                if path.is_dir() {
                    collect(root, &path, paths);
                } else {
                    paths.push(
                        path.strip_prefix(root)
                            .expect("fixture path is beneath root")
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        let mut paths = Vec::new();
        collect(&self.root, &self.root, &mut paths);
        paths.sort();
        paths
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove language reach fixture");
    }
}
