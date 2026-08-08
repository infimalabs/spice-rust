mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::jurisdiction::Ignore;

    use super::*;

    struct FixtureTree {
        root: PathBuf,
    }

    impl FixtureTree {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-hygiene-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create hygiene fixture root");
            Self { root }
        }

        fn write(&self, path: &str, contents: &str) {
            let target = self.root.join(path);
            fs::create_dir_all(target.parent().expect("fixture file has a parent"))
                .expect("create hygiene fixture parent");
            fs::write(target, contents).expect("write hygiene fixture source");
        }

        fn jurisdiction(&self) -> Jurisdiction {
            Jurisdiction::walk(&self.root, &Ignore::default()).expect("walk hygiene fixture tree")
        }

        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path)
        }
    }

    impl Drop for FixtureTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn configuration() -> HygieneConfiguration {
        HygieneConfiguration::new(["app/main.py"], ["tests"])
            .expect("fixture roots are admitted")
            .with_assertion_helpers(["expect_error"])
            .expect("fixture helper is admitted")
    }

    fn multifile_tree() -> FixtureTree {
        let tree = FixtureTree::new();
        tree.write("pyproject.toml", "[project]\nname = 'fixture'\n");
        tree.write("spice.toml", "[policy]\n");
        tree.write(
            "app/__init__.py",
            concat!(
                "from .used import live\n",
                "\n",
                "def package_live():\n",
                "    return live()\n",
            ),
        );
        tree.write(
            "app/nested/__init__.py",
            concat!(
                "from ..used import live\n",
                "\n",
                "def nested_live():\n",
                "    return live()\n",
            ),
        );
        tree.write(
            "app/main.py",
            concat!(
                "from app import package_live\n",
                "from app.nested import nested_live\n",
                "\n",
                "def entry():\n",
                "    return package_live() + nested_live()\n",
            ),
        );
        tree.write(
            "app/used.py",
            concat!(
                "def live():\n",
                "    return 1\n",
                "\n",
                "def test_only():\n",
                "    return 2\n",
                "\n",
                "def orphan():\n",
                "    return 3\n",
                "\n",
                "class Box:\n",
                "    def method(self):\n",
                "        return 4\n",
                "\n",
                "def dead_a():\n",
                "    return dead_b()\n",
                "\n",
                "def dead_b():\n",
                "    return dead_a()\n",
            ),
        );
        tree.write("app/test_support.py", "def build():\n    return object()\n");
        tree.write("app/_internal.py", "def _secret():\n    return 1\n");
        tree.write(
            "tests/test_main.py",
            concat!(
                "import app._internal as hidden\n",
                "from app import test_support\n",
                "from app.used import Box, live, test_only\n",
                "\n",
                "def test_live():\n",
                "    assert live() == 1\n",
                "    assert test_only() == 2\n",
                "\n",
                "def test_no_assert():\n",
                "    test_support.build()\n",
                "\n",
                "class TestPrivate:\n",
                "    def test_secret(self):\n",
                "        assert hidden._secret() == 1\n",
                "        Box().method()\n",
            ),
        );
        tree
    }

    fn clean_tree() -> FixtureTree {
        let tree = FixtureTree::new();
        tree.write("app/__init__.py", "");
        tree.write("app/used.py", "def live():\n    return 1\n");
        tree.write("app/used.pyi", "def live() -> int: ...\n");
        tree.write(
            "app/main.py",
            concat!(
                "from .used import live\n",
                "\n",
                "def entry():\n",
                "    return live()\n",
                "\n",
                "entry()\n",
            ),
        );
        tree.write(
            "tests/test_main.py",
            concat!(
                "from app.main import entry\n",
                "\n",
                "def test_entry():\n",
                "    assert entry() == 1\n",
            ),
        );
        tree
    }

    fn root_package_tree() -> FixtureTree {
        let tree = FixtureTree::new();
        tree.write(
            "__init__.py",
            concat!(
                "from .used import live\n",
                "\n",
                "def package_live():\n",
                "    return live()\n",
            ),
        );
        tree.write(
            "used.py",
            concat!(
                "def live():\n",
                "    return 1\n",
                "\n",
                "def test_only():\n",
                "    return 2\n",
            ),
        );
        tree.write(
            "tests/test_root.py",
            concat!(
                "from used import live, test_only\n",
                "\n",
                "def test_live():\n",
                "    assert live() == 1\n",
                "    assert test_only() == 2\n",
            ),
        );
        tree
    }

    /// One import chain, entered by one test at each end.
    ///
    /// `tests/test_middle.py` names the middle and reaches the leaf through
    /// it; `tests/test_leaf.py` names the leaf outright. The leaf therefore
    /// separates the two counts while the middle holds them equal, so a
    /// ranking that confused them could not report both rows.
    fn reach_chain_tree() -> FixtureTree {
        let tree = FixtureTree::new();
        tree.write("app/__init__.py", "");
        tree.write(
            "app/main.py",
            concat!(
                "from app.middle import passthrough\n",
                "\n",
                "def entry():\n",
                "    return passthrough()\n",
            ),
        );
        tree.write(
            "app/middle.py",
            concat!(
                "from app.leaf import deep\n",
                "\n",
                "def passthrough():\n",
                "    return deep()\n",
            ),
        );
        tree.write("app/leaf.py", "def deep():\n    return 1\n");
        tree.write(
            "tests/test_middle.py",
            concat!(
                "from app.middle import passthrough\n",
                "\n",
                "def test_passthrough():\n",
                "    assert passthrough() == 1\n",
            ),
        );
        tree.write(
            "tests/test_leaf.py",
            concat!(
                "from app.leaf import deep\n",
                "\n",
                "def test_deep():\n",
                "    assert deep() == 1\n",
            ),
        );
        tree
    }

    fn arm(report: &HygieneReport, gate: GateId) -> &HygieneMeasurement {
        report
            .arms
            .iter()
            .find(|arm| arm.gate_id == gate)
            .expect("report contains every hygiene arm")
    }

    fn graph_counts(measurement: &HygieneMeasurement) -> (u32, u32) {
        measurement
            .evidence
            .iter()
            .find_map(|item| match item {
                AnalysisEvidence::GraphConstruction { nodes, edges, .. } => Some((*nodes, *edges)),
                _ => None,
            })
            .expect("graph arm retains graph evidence")
    }

    fn graph_reach(measurement: &HygieneMeasurement) -> &[ModuleReach] {
        measurement
            .evidence
            .iter()
            .find_map(|item| match item {
                AnalysisEvidence::GraphConstruction { reach, .. } => Some(reach.as_slice()),
                _ => None,
            })
            .expect("graph arm retains graph evidence")
    }

    /// The suite reaches further than it imports, and the ranking says how far.
    ///
    /// `tests/test_middle.py` never names `app/leaf.py`, yet importing the
    /// middle binds it to the leaf just as tightly once the process runs. A
    /// count of direct imports alone would rank the leaf level with the middle
    /// and hide that changing the leaf leaves a test unrun by whoever reads
    /// only the tests that name it.
    #[test]
    fn the_reach_ranking_counts_a_test_that_reaches_a_module_without_naming_it() {
        let tree = reach_chain_tree();
        let report =
            analyze_hygiene(&tree.jurisdiction(), &configuration()).expect("reach chain report");

        assert_eq!(
            graph_reach(arm(&report, GateId::Reachability))
                .iter()
                .map(|entry| (entry.path.as_str(), entry.reached_by, entry.imported_by))
                .collect::<Vec<_>>(),
            [("app/leaf.py", 2, 1), ("app/middle.py", 1, 1)],
            "the leaf is reached by both tests and named by one"
        );
    }

    #[test]
    fn relative_imports_anchor_to_the_containing_package_identity() {
        for (path, current, initializer, level, expected) in [
            ("__init__.py", "", true, 1, "used"),
            ("__init__.pyi", "", true, 1, "used"),
            ("app/main.py", "app.main", false, 1, "app.used"),
            ("app/main.pyi", "app.main", false, 1, "app.used"),
            (
                "app/nested/main.py",
                "app.nested.main",
                false,
                2,
                "app.used",
            ),
            ("app/__init__.py", "app", true, 1, "app.used"),
            ("app/__init__.pyi", "app", true, 1, "app.used"),
            (
                "app/nested/__init__.py",
                "app.nested",
                true,
                1,
                "app.nested.used",
            ),
            ("app/nested/__init__.pyi", "app.nested", true, 2, "app.used"),
        ] {
            let parsed = Module::new(path.to_owned(), current.to_owned(), false, "");
            assert_eq!(parsed.is_package_initializer, initializer, "{path}");
            assert_eq!(
                resolve_relative_module(
                    &parsed.name,
                    parsed.is_package_initializer,
                    level,
                    Some("used".to_owned()),
                ),
                expected,
                "path={path} current={current} level={level}"
            );
        }
    }

    #[test]
    fn root_package_relative_imports_retain_known_modules_and_symbols() {
        let tree = root_package_tree();
        let configuration = HygieneConfiguration::new(["__init__.py"], ["tests"])
            .expect("root-package fixture roots are admitted");
        let jurisdiction = tree.jurisdiction();
        let (_, corpus, terminal) = Corpus::load(&jurisdiction, &configuration, Instant::now());
        assert!(matches!(terminal, Terminal::Parsed));
        assert!(corpus.edges.contains(&(String::new(), "used".to_owned())));
        assert!(corpus.resolved_references().iter().any(|reference| {
            reference.source_module.is_empty()
                && reference.target_module == "used"
                && reference.symbol == "live"
        }));

        let report = analyze_hygiene(&jurisdiction, &configuration)
            .expect("root-package source-derived hygiene report");
        let reachability = arm(&report, GateId::Reachability);
        assert!(!reachability.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::ModuleReachability { subject, .. }
                    if subject == "used"
            )
        }));
        let symbols = arm(&report, GateId::SymbolReachability);
        assert!(!symbols.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::SymbolReachability { symbol, .. }
                    if symbol == "live"
            )
        }));
        let unused = arm(&report, GateId::PythonUnused);
        assert!(!unused.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::PythonUnused { symbol, .. }
                    if symbol == "live"
            )
        }));
        assert!(unused.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::PythonUnused {
                    symbol,
                    status: PythonUnusedStatus::TestOnly,
                    ..
                } if symbol == "test_only"
            )
        }));
    }

    fn assert_relative_package_graph(corpus: &Corpus) {
        let references = corpus.resolved_references();
        for source in ["app", "app.nested"] {
            assert!(
                corpus
                    .edges
                    .contains(&(source.to_owned(), "app.used".to_owned())),
                "{source} keeps its relative import edge to app.used"
            );
            assert!(references.iter().any(|reference| {
                reference.source_module == source
                    && reference.target_module == "app.used"
                    && reference.symbol == "live"
            }));
        }
    }

    fn assert_multifile_graph(report: &HygieneReport) {
        assert_eq!(
            report
                .arms
                .iter()
                .map(|measurement| measurement.gate_id)
                .collect::<Vec<_>>(),
            HYGIENE_ARMS.map(|declaration| declaration.id)
        );
        assert_eq!(graph_counts(arm(report, GateId::Reachability)), (7, 7));
        assert_eq!(
            graph_counts(arm(report, GateId::SymbolReachability)),
            (7, 7)
        );
    }

    fn assert_reachability_and_symbol_findings(report: &HygieneReport) {
        let reachability = arm(report, GateId::Reachability);
        assert_eq!(reachability.outcome, HygieneOutcome::Findings);
        assert!(reachability.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::ModuleReachability { subject, .. }
                    if subject == "app.test_support"
            )
        }));

        let symbols = arm(report, GateId::SymbolReachability);
        assert_eq!(symbols.outcome, HygieneOutcome::Findings);
        assert!(symbols.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::SymbolReachability { symbol, .. }
                    if symbol == "test_only"
            )
        }));
        for symbol in ["live", "package_live", "nested_live"] {
            assert!(!symbols.findings.iter().any(|finding| {
                matches!(
                    &finding.detail,
                    HygieneFindingDetail::SymbolReachability {
                        symbol: observed,
                        ..
                    } if observed == symbol
                )
            }));
        }
    }

    fn assert_unused_findings(report: &HygieneReport) {
        let unused = arm(report, GateId::PythonUnused);
        assert_eq!(unused.outcome, HygieneOutcome::Findings);
        for symbol in ["live", "package_live", "nested_live"] {
            assert!(!unused.findings.iter().any(|finding| {
                matches!(
                    &finding.detail,
                    HygieneFindingDetail::PythonUnused {
                        symbol: observed,
                        ..
                    } if observed == symbol
                )
            }));
        }
        for symbol in ["orphan", "dead_a", "dead_b"] {
            assert!(unused.findings.iter().any(|finding| {
                matches!(
                    &finding.detail,
                    HygieneFindingDetail::PythonUnused {
                        symbol: observed,
                        status: PythonUnusedStatus::CandidateUnused,
                        ..
                    } if observed == symbol
                )
            }));
        }
        assert!(unused.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::PythonUnused {
                    symbol,
                    status: PythonUnusedStatus::TestOnly,
                    ..
                } if symbol == "test_only"
            )
        }));
    }

    fn assert_test_policy_findings(report: &HygieneReport) {
        let assertions = arm(report, GateId::AssertionFreeTests);
        assert_eq!(assertions.outcome, HygieneOutcome::Findings);
        assert!(assertions.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::AssertionFree { test_name }
                    if test_name == "test_no_assert"
            )
        }));

        let private = arm(report, GateId::PrivateInternals);
        assert_eq!(private.outcome, HygieneOutcome::Findings);
        assert!(private.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::PrivateCoupling {
                    coupling_kind,
                    target,
                    ..
                } if coupling_kind == "private assertion"
                    && target == "app._internal._secret"
            )
        }));
    }

    #[test]
    fn native_run_derives_all_five_arms_from_multifile_python() {
        let tree = multifile_tree();
        let jurisdiction = tree.jurisdiction();
        let (_, corpus, terminal) = Corpus::load(&jurisdiction, &configuration(), Instant::now());
        assert!(matches!(terminal, Terminal::Parsed));
        assert_relative_package_graph(&corpus);
        let report = analyze_hygiene(&jurisdiction, &configuration())
            .expect("source-derived hygiene report");
        assert_multifile_graph(&report);
        assert_reachability_and_symbol_findings(&report);
        assert_unused_findings(&report);
        assert_test_policy_findings(&report);

        let clean = clean_tree();
        let clean_report = analyze_hygiene(&clean.jurisdiction(), &configuration())
            .expect("clean source-derived hygiene report");
        assert!(clean_report.arms.iter().all(|measurement| {
            measurement.outcome == HygieneOutcome::Passed && measurement.findings.is_empty()
        }));
    }

    #[test]
    fn native_boundaries_derive_pass_applicability_exclusion_parse_and_failure() {
        let tree = multifile_tree();
        let jurisdiction = tree.jurisdiction();
        let generous = configuration().with_debt_limits(u32::MAX, u32::MAX);
        let report = analyze_hygiene(&jurisdiction, &generous).expect("generous policy report");
        assert_eq!(
            arm(&report, GateId::Reachability).outcome,
            HygieneOutcome::Passed
        );
        assert_eq!(
            arm(&report, GateId::AssertionFreeTests).outcome,
            HygieneOutcome::Passed
        );

        let empty = FixtureTree::new();
        let empty_report = analyze_hygiene(&empty.jurisdiction(), &configuration())
            .expect("empty tree has typed applicability outcomes");
        assert!(
            empty_report
                .arms
                .iter()
                .all(|measurement| measurement.outcome == HygieneOutcome::NotApplicable)
        );

        let exclusion = HygieneExclusion::new(
            GateId::PrivateInternals,
            HygieneProvenance {
                release: "current".to_owned(),
                layer: "repository".to_owned(),
                document: "spice.toml".to_owned(),
                field: "policy.exclude[0]".to_owned(),
            },
            "tracked fixture exclusion",
        )
        .expect("fixture exclusion is admitted");
        let excluded = configuration()
            .with_exclusions([exclusion])
            .expect("one exclusion per gate");
        let excluded_report =
            analyze_hygiene(&jurisdiction, &excluded).expect("excluded source-derived report");
        assert_eq!(
            arm(&excluded_report, GateId::PrivateInternals).outcome,
            HygieneOutcome::Excluded
        );

        tree.write("app/broken.py", "def broken(:\n");
        let broken_report = analyze_hygiene(&tree.jurisdiction(), &configuration())
            .expect("parse failure remains report evidence");
        assert!(
            broken_report
                .arms
                .iter()
                .all(|measurement| measurement.outcome == HygieneOutcome::ParseFailure)
        );

        let failed_tree = multifile_tree();
        let failed_jurisdiction = failed_tree.jurisdiction();
        fs::remove_file(failed_tree.path("app/main.py")).expect("remove measured fixture file");
        let failed_report = analyze_hygiene(&failed_jurisdiction, &configuration())
            .expect("read failure remains report evidence");
        assert!(
            failed_report
                .arms
                .iter()
                .all(|measurement| measurement.outcome == HygieneOutcome::Failed)
        );

        let slow_tree = FixtureTree::new();
        let slow_source = "value = 1\n".repeat(5_000);
        slow_tree.write("app/main.py", &slow_source);
        slow_tree.write("tests/test_main.py", "def test_value():\n    assert True\n");
        let deadline = configuration()
            .with_deadline_ms(1)
            .expect("nonzero fixture deadline");
        let timeout_report = analyze_hygiene(&slow_tree.jurisdiction(), &deadline)
            .expect("deadline remains report evidence");
        assert!(
            timeout_report
                .arms
                .iter()
                .all(|measurement| measurement.outcome == HygieneOutcome::Timeout)
        );
    }

    #[test]
    fn tracked_private_exceptions_are_applied_to_source_derived_couplings() {
        let tree = multifile_tree();
        let jurisdiction = tree.jurisdiction();
        let admitted =
            PrivateCouplingException::new("TestPrivate.test_secret", "app._internal._secret")
                .expect("fixture exception");
        let stale = PrivateCouplingException::new("test_missing", "app._internal._missing")
            .expect("stale fixture exception");
        let configured = configuration().with_private_exceptions([admitted, stale]);
        let report = analyze_hygiene(&jurisdiction, &configured).expect("private exception report");
        let private = arm(&report, GateId::PrivateInternals);
        assert!(!private.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::PrivateCoupling { target, .. }
                    if target == "app._internal._secret"
            )
        }));
        assert!(private.findings.iter().any(|finding| {
            matches!(
                &finding.detail,
                HygieneFindingDetail::StalePrivateException { test_name, target }
                    if test_name == "test_missing" && target == "app._internal._missing"
            )
        }));
    }

    #[test]
    fn a_suppression_missing_any_part_of_its_origin_is_refused_at_admission() {
        let complete = SuppressedPath {
            path: "tests/test_ignored.py".to_owned(),
            shape: "file".to_owned(),
            rule: "tests/test_ignored.py".to_owned(),
            provenance: HygieneProvenance {
                release: "current".to_owned(),
                layer: "repository".to_owned(),
                document: "spice.toml".to_owned(),
                field: "gate.walk.skip[0]".to_owned(),
            },
        };
        configuration()
            .with_suppressions([complete.clone()])
            .expect("a complete suppression is admitted");

        for blank in [
            |item: &mut SuppressedPath| item.path.clear(),
            |item: &mut SuppressedPath| item.shape.clear(),
            |item: &mut SuppressedPath| item.rule.clear(),
            |item: &mut SuppressedPath| item.provenance.release.clear(),
            |item: &mut SuppressedPath| item.provenance.layer.clear(),
            |item: &mut SuppressedPath| item.provenance.document.clear(),
            |item: &mut SuppressedPath| item.provenance.field.clear(),
        ] {
            let mut incomplete = complete.clone();
            blank(&mut incomplete);
            assert!(
                matches!(
                    configuration().with_suppressions([incomplete.clone()]),
                    Err(HygieneAnalysisError::Configuration(_))
                ),
                "{incomplete:?} is not auditable and must not be admitted"
            );
        }
    }

    #[derive(Clone, Copy)]
    enum BoundaryState {
        Available,
        Unavailable,
        Unsupported,
        Unmeasured,
    }

    struct FixtureBoundary {
        gate: GateId,
        state: BoundaryState,
    }

    impl ExecutionBoundary for FixtureBoundary {
        fn scheduled(&self, gate: GateId) -> bool {
            gate != self.gate || !matches!(self.state, BoundaryState::Unmeasured)
        }

        fn analyzer(&self, declaration: HygieneDeclaration) -> AnalyzerEvidence {
            if declaration.id != self.gate || matches!(self.state, BoundaryState::Available) {
                return NativeExecution.analyzer(declaration);
            }
            match self.state {
                BoundaryState::Unavailable => AnalyzerEvidence::Unavailable {
                    name: declaration.analyzer.evidence_name().to_owned(),
                    requested: declaration.analyzer.component().to_owned(),
                },
                BoundaryState::Unsupported => AnalyzerEvidence::Identified {
                    name: declaration.analyzer.evidence_name().to_owned(),
                    requested: declaration.analyzer.component().to_owned(),
                    resolved: declaration.analyzer.component().to_owned(),
                    declared_version: NATIVE_ANALYZER_VERSION.to_owned(),
                    observed_version: "spice-gate-hygiene-v0".to_owned(),
                    digest: "fixture-observed-native-digest".to_owned(),
                },
                BoundaryState::Available | BoundaryState::Unmeasured => {
                    NativeExecution.analyzer(declaration)
                }
            }
        }
    }

    #[test]
    fn analyzer_and_scheduler_boundaries_derive_discovery_states() {
        let tree = multifile_tree();
        let jurisdiction = tree.jurisdiction();
        for (state, outcome) in [
            (BoundaryState::Unavailable, HygieneOutcome::Unavailable),
            (BoundaryState::Unsupported, HygieneOutcome::Unsupported),
            (BoundaryState::Unmeasured, HygieneOutcome::Unmeasured),
        ] {
            let boundary = FixtureBoundary {
                gate: GateId::Reachability,
                state,
            };
            let report = AnalyzerRun::execute_with(&jurisdiction, &configuration(), &boundary)
                .expect("boundary-derived report");
            assert_eq!(arm(&report, GateId::Reachability).outcome, outcome);
        }

        let available = FixtureBoundary {
            gate: GateId::Reachability,
            state: BoundaryState::Available,
        };
        let report = AnalyzerRun::execute_with(&jurisdiction, &configuration(), &available)
            .expect("available boundary executes source analyzers");
        assert_eq!(
            arm(&report, GateId::Reachability).outcome,
            HygieneOutcome::Findings
        );
    }
}
