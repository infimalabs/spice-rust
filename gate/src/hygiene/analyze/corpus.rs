use super::*;

const GRAPH_SYMBOL_LIMIT: usize = 1_000_000;
const GRAPH_EDGE_LIMIT: usize = 1_000_000;
const GRAPH_WORKLIST_LIMIT: usize = 1_000_000;
const GRAPH_FINDING_LIMIT: usize = 1_000_000;
const GRAPH_DIAGNOSTIC_BYTES_LIMIT: usize = 64 * 1024 * 1024;
const GRAPH_STEP_LIMIT: usize = 10_000_000;
const _: () = assert!(
    GRAPH_SYMBOL_LIMIT > 0
        && GRAPH_EDGE_LIMIT > 0
        && GRAPH_WORKLIST_LIMIT > 0
        && GRAPH_FINDING_LIMIT > 0
        && GRAPH_DIAGNOSTIC_BYTES_LIMIT > 0
        && GRAPH_STEP_LIMIT > 0
);

#[derive(Clone, Copy)]
pub(super) enum GraphFailure {
    Symbols,
    Edges,
    Worklist,
    Findings,
    Diagnostics,
    Steps,
    Deadline,
}

pub(super) struct GraphWork {
    started: Instant,
    deadline_ms: u64,
    edges: usize,
    findings: usize,
    diagnostic_bytes: usize,
    steps: usize,
    failure: Option<GraphFailure>,
}

impl GraphWork {
    pub(super) fn new(started: Instant, deadline_ms: u64) -> Self {
        Self {
            started,
            deadline_ms,
            edges: 0,
            findings: 0,
            diagnostic_bytes: 0,
            steps: 0,
            failure: None,
        }
    }

    pub(super) fn preflight(
        &mut self,
        corpus: &Corpus,
        configuration: &HygieneConfiguration,
    ) -> bool {
        let symbols = corpus.modules.values().fold(0_usize, |total, module| {
            total
                .saturating_add(module.imports.len())
                .saturating_add(module.declarations.len())
                .saturating_add(module.references.len())
                .saturating_add(module.tests.len())
                .saturating_add(module.private_couplings.len())
        });
        if symbols > GRAPH_SYMBOL_LIMIT {
            self.failure = Some(GraphFailure::Symbols);
            return false;
        }
        let possible_findings = corpus
            .modules
            .len()
            .saturating_add(
                corpus
                    .modules
                    .values()
                    .fold(0_usize, |total, module| {
                        total.saturating_add(module.declarations.len())
                    })
                    .saturating_mul(2),
            )
            .saturating_add(corpus.modules.values().fold(0_usize, |total, module| {
                total
                    .saturating_add(module.tests.len())
                    .saturating_add(module.private_couplings.len())
            }))
            .saturating_add(configuration.private_exceptions.len());
        if possible_findings > GRAPH_FINDING_LIMIT {
            self.failure = Some(GraphFailure::Findings);
            return false;
        }
        self.check()
    }

    fn check(&mut self) -> bool {
        if self.failure.is_some() {
            return false;
        }
        if u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX) > self.deadline_ms
        {
            self.failure = Some(GraphFailure::Deadline);
            false
        } else {
            true
        }
    }

    fn step(&mut self) -> bool {
        self.steps(1)
    }

    pub(super) fn steps(&mut self, count: usize) -> bool {
        if !self.check() {
            return false;
        }
        let next = self.steps.saturating_add(count);
        if next > GRAPH_STEP_LIMIT {
            self.failure.get_or_insert(GraphFailure::Steps);
            return false;
        }
        self.steps = next;
        true
    }

    fn admit_edge(&mut self) -> bool {
        if !self.step() || self.edges == GRAPH_EDGE_LIMIT {
            self.failure.get_or_insert(GraphFailure::Edges);
            return false;
        }
        self.edges += 1;
        true
    }

    fn admit_worklist(&mut self, length: usize) -> bool {
        if length >= GRAPH_WORKLIST_LIMIT {
            self.failure = Some(GraphFailure::Worklist);
            false
        } else {
            self.step()
        }
    }

    fn admit_finding(&mut self, bytes: usize) -> bool {
        if !self.step() || self.findings == GRAPH_FINDING_LIMIT {
            self.failure.get_or_insert(GraphFailure::Findings);
            return false;
        }
        let next = self.diagnostic_bytes.saturating_add(bytes);
        if next > GRAPH_DIAGNOSTIC_BYTES_LIMIT {
            self.failure = Some(GraphFailure::Diagnostics);
            return false;
        }
        self.findings += 1;
        self.diagnostic_bytes = next;
        true
    }

    pub(super) const fn failure(&self) -> Option<GraphFailure> {
        self.failure
    }

    pub(super) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub(super) fn diagnostic(&self) -> String {
        let bound = match self.failure {
            Some(GraphFailure::Symbols) => "symbol",
            Some(GraphFailure::Edges) => "edge",
            Some(GraphFailure::Worklist) => "worklist",
            Some(GraphFailure::Findings) => "finding",
            Some(GraphFailure::Diagnostics) => "diagnostic-byte",
            Some(GraphFailure::Steps) => "graph-step",
            Some(GraphFailure::Deadline) => "deadline",
            None => "completed",
        };
        format!(
            "hygiene graph {bound} bound exhausted after {} steps",
            self.steps
        )
    }
}

#[derive(Default)]
pub(super) struct Corpus {
    modules: BTreeMap<String, Module>,
    pub(super) edges: BTreeSet<(String, String)>,
    production_roots: BTreeSet<String>,
    test_modules: BTreeSet<String>,
    production_reachable: BTreeSet<String>,
    test_reachable: BTreeSet<String>,
}

impl Corpus {
    pub(super) fn load(
        documents: &[HygieneDocument],
        configuration: &HygieneConfiguration,
        started: Instant,
        reached_paths: Option<&BTreeSet<String>>,
    ) -> (u32, Self, Terminal) {
        let documents = documents
            .iter()
            .filter(|document| reached_paths.is_none_or(|reached| reached.contains(&document.path)))
            .collect::<Vec<_>>();
        let selected_files = u32::try_from(documents.len()).unwrap_or(u32::MAX);
        let mut corpus = Self::default();
        let mut ast_work = AstWork::new(started, configuration.deadline_ms);
        for (index, document) in documents.iter().enumerate() {
            let path = &document.path;
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if elapsed_ms > configuration.deadline_ms {
                return (selected_files, corpus, Terminal::TimedOut { elapsed_ms });
            }
            let source = match std::str::from_utf8(&document.bytes) {
                Ok(source) => source,
                Err(error) => {
                    return (
                        selected_files,
                        corpus,
                        Terminal::ParseFailure {
                            documents: u32::try_from(index).unwrap_or(u32::MAX),
                            path: path.clone(),
                            diagnostic: format!("source is not UTF-8: {error}"),
                        },
                    );
                }
            };
            let suite = match rustpython_ast::Suite::parse(source, path) {
                Ok(suite) => suite,
                Err(error) => {
                    return (
                        selected_files,
                        corpus,
                        Terminal::ParseFailure {
                            documents: u32::try_from(index).unwrap_or(u32::MAX),
                            path: path.clone(),
                            diagnostic: error.to_string(),
                        },
                    );
                }
            };
            let name = module_name(path);
            let is_test = path_is_under_roots(path, &configuration.test_roots);
            let Some(mut module) =
                Module::new(path.clone(), name.clone(), is_test, source, &mut ast_work)
            else {
                return ast_terminal(selected_files, corpus, index, path, &ast_work);
            };
            {
                let mut collector = AstCollector::new(&mut module, configuration, &mut ast_work);
                for statement in suite {
                    collector.visit_stmt(statement);
                }
            }
            if ast_work.failure().is_some() {
                return ast_terminal(selected_files, corpus, index, path, &ast_work);
            }
            if let Some(existing_path) = corpus.modules.get(&name).map(|item| item.path.clone()) {
                if existing_path.ends_with(".py") && path.ends_with(".pyi") {
                    continue;
                }
                if !(existing_path.ends_with(".pyi") && path.ends_with(".py")) {
                    return (
                        selected_files,
                        corpus,
                        Terminal::ParseFailure {
                            documents: u32::try_from(index).unwrap_or(u32::MAX),
                            path: path.clone(),
                            diagnostic: format!(
                                concat!(
                                    "selected paths {} and {} resolve to the same ",
                                    "Python module {}"
                                ),
                                existing_path, path, name
                            ),
                        },
                    );
                }
            }
            corpus.modules.insert(name, module);
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if elapsed_ms > configuration.deadline_ms {
                return (selected_files, corpus, Terminal::TimedOut { elapsed_ms });
            }
        }
        let mut graph_work = GraphWork::new(started, configuration.deadline_ms);
        if !graph_work.preflight(&corpus, configuration)
            || !corpus.construct_graphs(configuration, &mut graph_work)
        {
            return graph_terminal(selected_files, corpus, &graph_work);
        }
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let terminal = if elapsed_ms > configuration.deadline_ms {
            Terminal::TimedOut { elapsed_ms }
        } else {
            Terminal::Parsed
        };
        (selected_files, corpus, terminal)
    }

    fn construct_graphs(
        &mut self,
        configuration: &HygieneConfiguration,
        work: &mut GraphWork,
    ) -> bool {
        let known: BTreeSet<_> = self.modules.keys().cloned().collect();
        for (name, module) in &self.modules {
            if !work.step() {
                return false;
            }
            if module.is_test {
                self.test_modules.insert(name.clone());
            }
            if !module.is_test && path_is_under_roots(&module.path, &configuration.production_roots)
            {
                self.production_roots.insert(name.clone());
            }
            for import in &module.imports {
                if !work.step() {
                    return false;
                }
                if let Some(target) = import.target_module(&known) {
                    let edge = (name.clone(), target);
                    if !self.edges.contains(&edge) {
                        if !work.admit_edge() {
                            return false;
                        }
                        self.edges.insert(edge);
                    }
                }
            }
        }
        let Some(production_reachable) =
            reachable_bounded(&self.production_roots, &self.edges, work)
        else {
            return false;
        };
        let Some(test_reachable) = reachable_bounded(&self.test_modules, &self.edges, work) else {
            return false;
        };
        self.production_reachable = production_reachable;
        self.test_reachable = test_reachable;
        true
    }

    pub(super) fn applicable(&self, gate: GateId) -> bool {
        let graph = !self.modules.is_empty()
            && !self.production_roots.is_empty()
            && !self.test_modules.is_empty();
        let declarations = self
            .declarations()
            .any(|(_, declaration)| declaration.top_level);
        let tests = self.tests().next().is_some();
        match gate {
            GateId::Reachability => graph,
            GateId::SymbolReachability | GateId::PythonUnused => graph && declarations,
            GateId::AssertionFreeTests => tests,
            GateId::PrivateInternals => tests && !self.production_packages().is_empty(),
            _ => false,
        }
    }

    pub(super) fn traversal_evidence(
        &self,
        traversal: HygieneTraversal,
        configuration: &HygieneConfiguration,
        focused: bool,
    ) -> AnalysisEvidence {
        match traversal {
            HygieneTraversal::ImportGraph => AnalysisEvidence::GraphConstruction {
                production_roots: self.root_paths(&self.production_roots),
                test_roots: self.root_paths(&self.test_modules),
                nodes: u32::try_from(self.modules.len()).unwrap_or(u32::MAX),
                edges: u32::try_from(self.edges.len()).unwrap_or(u32::MAX),
                reach: self.suite_reach(),
            },
            HygieneTraversal::SymbolGraph | HygieneTraversal::PythonDeclarations => {
                let resolved = self.resolved_references();
                let reachable = self.production_symbols(&resolved);
                AnalysisEvidence::SymbolExtraction {
                    declarations: u32::try_from(self.declarations().count()).unwrap_or(u32::MAX),
                    production_references: u32::try_from(
                        resolved
                            .iter()
                            .filter(|item| {
                                !item.from_test
                                    && item.source_symbol.as_ref().is_none_or(|symbol| {
                                        reachable
                                            .contains(&(item.source_module.clone(), symbol.clone()))
                                    })
                            })
                            .count(),
                    )
                    .unwrap_or(u32::MAX),
                    test_references: u32::try_from(
                        resolved.iter().filter(|item| item.from_test).count(),
                    )
                    .unwrap_or(u32::MAX),
                }
            }
            HygieneTraversal::TestAssertions => AnalysisEvidence::TestClassification {
                files: u32::try_from(
                    self.modules
                        .values()
                        .filter(|module| module.is_test)
                        .count(),
                )
                .unwrap_or(u32::MAX),
                tests: u32::try_from(self.tests().count()).unwrap_or(u32::MAX),
                assertions: u32::try_from(
                    self.tests().filter(|(_, test)| test.assertions > 0).count(),
                )
                .unwrap_or(u32::MAX),
                helpers: configuration.assertion_helpers.clone(),
            },
            HygieneTraversal::PrivateBoundaries => {
                let couplings = self.private_couplings();
                let admitted = couplings
                    .iter()
                    .filter(|coupling| configuration.exception_matches(coupling))
                    .count();
                let stale = configuration
                    .private_exceptions
                    .iter()
                    .filter(|exception| !focused || self.private_exception_relevant(exception))
                    .filter(|exception| !couplings.iter().any(|item| exception.matches(item)))
                    .count();
                AnalysisEvidence::PrivateBoundaryResolution {
                    packages: self.production_packages().into_iter().collect(),
                    private_imports: u32::try_from(
                        couplings
                            .iter()
                            .filter(|item| item.kind == "private import")
                            .count(),
                    )
                    .unwrap_or(u32::MAX),
                    private_assertions: u32::try_from(
                        couplings
                            .iter()
                            .filter(|item| item.kind == "private assertion")
                            .count(),
                    )
                    .unwrap_or(u32::MAX),
                    admitted_exceptions: u32::try_from(admitted).unwrap_or(u32::MAX),
                    stale_exceptions: u32::try_from(stale).unwrap_or(u32::MAX),
                }
            }
        }
    }

    pub(super) fn findings(
        &self,
        gate: GateId,
        configuration: &HygieneConfiguration,
        focused: bool,
        work: &mut GraphWork,
    ) -> Option<Vec<HygieneFinding>> {
        let declarations = self.declarations().count();
        let references = self.modules.values().fold(0_usize, |total, module| {
            total.saturating_add(module.references.len())
        });
        let estimate = match gate {
            GateId::Reachability => self.modules.len().saturating_mul(self.edges.len()),
            GateId::SymbolReachability | GateId::PythonUnused => declarations
                .saturating_mul(references.max(1))
                .saturating_mul(declarations.max(1)),
            GateId::AssertionFreeTests => self.tests().count(),
            GateId::PrivateInternals => self
                .modules
                .values()
                .map(|module| module.private_couplings.len())
                .sum::<usize>()
                .saturating_add(configuration.private_exceptions.len()),
            _ => 0,
        };
        if !work.steps(estimate) {
            return None;
        }
        let findings = match gate {
            GateId::Reachability => self.reachability_findings(),
            GateId::SymbolReachability => self.symbol_reachability_findings(),
            GateId::PythonUnused => self.python_unused_findings(),
            GateId::AssertionFreeTests => self.assertion_free_findings(),
            GateId::PrivateInternals => self.private_findings(configuration, focused),
            _ => Vec::new(),
        };
        for finding in &findings {
            if !work.admit_finding(finding_bytes(finding)) {
                return None;
            }
        }
        Some(findings)
    }

    pub(super) fn traversal_steps(&self, traversal: HygieneTraversal) -> usize {
        let declarations = self.declarations().count();
        let references = self.modules.values().fold(0_usize, |total, module| {
            total.saturating_add(module.references.len())
        });
        match traversal {
            HygieneTraversal::ImportGraph => self
                .test_modules
                .len()
                .max(2)
                .saturating_mul(self.modules.len().saturating_add(self.edges.len())),
            HygieneTraversal::SymbolGraph | HygieneTraversal::PythonDeclarations => declarations
                .saturating_mul(references.max(1))
                .saturating_mul(declarations.max(1)),
            HygieneTraversal::TestAssertions => self.tests().count(),
            HygieneTraversal::PrivateBoundaries => self.modules.len().saturating_mul(
                self.modules
                    .values()
                    .fold(0_usize, |total, module| {
                        total.saturating_add(module.private_couplings.len())
                    })
                    .max(1),
            ),
        }
    }

    fn reachability_findings(&self) -> Vec<HygieneFinding> {
        self.test_reachable
            .difference(&self.production_reachable)
            .filter(|name| !self.test_modules.contains(*name))
            .filter_map(|name| {
                let module = self.modules.get(name)?;
                let imported_by = self
                    .edges
                    .iter()
                    .filter(|(from, to)| to == name && self.test_reachable.contains(from))
                    .filter_map(|(from, _)| self.modules.get(from).map(|item| item.path.clone()))
                    .collect();
                Some(HygieneFinding {
                    path: module.path.clone(),
                    line: None,
                    detail: HygieneFindingDetail::ModuleReachability {
                        provider: "python".to_owned(),
                        subject: name.clone(),
                        imported_by,
                    },
                })
            })
            .collect()
    }

    fn symbol_reachability_findings(&self) -> Vec<HygieneFinding> {
        let references = self.resolved_references();
        let production = self.production_symbols(&references);
        self.declarations()
            .filter_map(|(module, declaration)| {
                let production_reachable =
                    production.contains(&(module.name.clone(), declaration.name.clone()));
                let mut imported_by: Vec<_> = references
                    .iter()
                    .filter(|reference| {
                        reference.from_test && reference.matches(&module.name, declaration)
                    })
                    .map(|reference| reference.path.clone())
                    .collect();
                imported_by.sort();
                imported_by.dedup();
                if production_reachable || imported_by.is_empty() {
                    return None;
                }
                Some(HygieneFinding {
                    path: module.path.clone(),
                    line: Some(declaration.line),
                    detail: HygieneFindingDetail::SymbolReachability {
                        provider: "python".to_owned(),
                        module: module.name.clone(),
                        symbol: declaration.name.clone(),
                        symbol_kind: declaration.kind.to_owned(),
                        imported_by,
                    },
                })
            })
            .collect()
    }

    fn python_unused_findings(&self) -> Vec<HygieneFinding> {
        let references = self.resolved_references();
        let production = self.production_symbols(&references);
        self.declarations()
            .filter(|(_, declaration)| declaration.top_level)
            .filter_map(|(module, declaration)| {
                if production.contains(&(module.name.clone(), declaration.name.clone())) {
                    return None;
                }
                let mut test_references: Vec<_> = references
                    .iter()
                    .filter(|reference| {
                        reference.from_test && reference.matches(&module.name, declaration)
                    })
                    .map(|reference| reference.path.clone())
                    .collect();
                test_references.sort();
                test_references.dedup();
                let (status, reason) = if test_references.is_empty() {
                    (
                        PythonUnusedStatus::CandidateUnused,
                        "no production or test references",
                    )
                } else {
                    (PythonUnusedStatus::TestOnly, "references only in tests")
                };
                Some(HygieneFinding {
                    path: module.path.clone(),
                    line: Some(declaration.line),
                    detail: HygieneFindingDetail::PythonUnused {
                        module: module.name.clone(),
                        symbol: declaration.name.clone(),
                        declaration_kind: declaration.kind.to_owned(),
                        status,
                        reason: reason.to_owned(),
                        test_references,
                    },
                })
            })
            .collect()
    }

    fn assertion_free_findings(&self) -> Vec<HygieneFinding> {
        self.tests()
            .filter(|(_, test)| test.assertions == 0)
            .map(|(module, test)| HygieneFinding {
                path: module.path.clone(),
                line: Some(test.line),
                detail: HygieneFindingDetail::AssertionFree {
                    test_name: test.name.clone(),
                },
            })
            .collect()
    }

    fn private_findings(
        &self,
        configuration: &HygieneConfiguration,
        focused: bool,
    ) -> Vec<HygieneFinding> {
        let couplings = self.private_couplings();
        let mut findings: Vec<_> = couplings
            .iter()
            .filter(|coupling| !configuration.exception_matches(coupling))
            .map(|coupling| HygieneFinding {
                path: coupling.path.clone(),
                line: Some(coupling.line),
                detail: HygieneFindingDetail::PrivateCoupling {
                    test_name: coupling.test_name.clone(),
                    coupling_kind: coupling.kind.to_owned(),
                    target: coupling.target.clone(),
                },
            })
            .collect();
        findings.extend(
            configuration
                .private_exceptions
                .iter()
                .filter(|exception| !focused || self.private_exception_relevant(exception))
                .filter(|exception| !couplings.iter().any(|item| exception.matches(item)))
                .map(|exception| HygieneFinding {
                    path: "spice-rust.toml".to_owned(),
                    line: None,
                    detail: HygieneFindingDetail::StalePrivateException {
                        test_name: exception.test_name.clone(),
                        target: exception.target.clone(),
                    },
                }),
        );
        findings
    }

    fn private_exception_relevant(&self, exception: &PrivateCouplingException) -> bool {
        self.tests()
            .any(|(_, test)| test.name == exception.test_name)
            || self.modules.keys().any(|module| {
                exception.target == *module
                    || exception
                        .target
                        .strip_prefix(module)
                        .is_some_and(|tail| tail.starts_with('.'))
            })
    }

    /// Rank the modules the suite reaches by how much of the suite reaches them.
    ///
    /// One traversal per test module rather than the single traversal from all
    /// of them the arms already hold: that union answers whether anything
    /// reaches a module, and this answers how much, which is the number that
    /// separates a module a lane can verify from one whose change leaves the
    /// rest of the suite unrun. Modules nothing reaches are absent rather than
    /// ranked at zero, because a module no test reaches never widens a run.
    fn suite_reach(&self) -> Vec<ModuleReach> {
        let mut reached: BTreeMap<String, u32> = BTreeMap::new();
        let mut imported: BTreeMap<String, u32> = BTreeMap::new();
        for test in &self.test_modules {
            let direct: BTreeSet<String> = self
                .edges
                .iter()
                .filter(|(from, _)| from == test)
                .map(|(_, to)| to.clone())
                .collect();
            for module in &direct {
                *imported.entry(module.clone()).or_default() += 1;
            }
            // Seeded with the direct imports rather than the test itself, so a
            // test module never counts as reaching itself, and seeded as a set
            // so one test importing a module twice still counts once.
            for module in reachable(&direct, &self.edges) {
                *reached.entry(module).or_default() += 1;
            }
        }
        let mut ranked: Vec<_> = reached
            .into_iter()
            .filter(|(name, _)| !self.test_modules.contains(name))
            .filter_map(|(name, reached_by)| {
                let module = self.modules.get(&name)?;
                Some(ModuleReach {
                    path: module.path.clone(),
                    reached_by,
                    imported_by: imported.get(&name).copied().unwrap_or_default(),
                })
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .reached_by
                .cmp(&left.reached_by)
                .then_with(|| left.path.cmp(&right.path))
        });
        ranked
    }

    fn root_paths(&self, roots: &BTreeSet<String>) -> Vec<String> {
        roots
            .iter()
            .filter_map(|name| self.modules.get(name).map(|module| module.path.clone()))
            .collect()
    }

    fn declarations(&self) -> impl Iterator<Item = (&Module, &Declaration)> {
        self.modules
            .values()
            .filter(|module| self.production_reachable.contains(&module.name))
            .flat_map(|module| module.declarations.iter().map(move |item| (module, item)))
    }

    fn tests(&self) -> impl Iterator<Item = (&Module, &TestCase)> {
        self.modules
            .values()
            .flat_map(|module| module.tests.iter().map(move |item| (module, item)))
    }

    fn production_packages(&self) -> BTreeSet<String> {
        self.production_reachable
            .iter()
            .filter_map(|module| {
                module
                    .split('.')
                    .next()
                    .filter(|package| !package.is_empty())
                    .map(str::to_owned)
            })
            .collect()
    }

    pub(super) fn resolved_references(&self) -> Vec<ResolvedReference> {
        let known: BTreeSet<_> = self.modules.keys().cloned().collect();
        let declarations: BTreeSet<_> = self
            .declarations()
            .map(|(module, declaration)| (module.name.clone(), declaration.name.clone()))
            .collect();
        let mut output = BTreeSet::new();
        for module in self.modules.values() {
            let from_test = self.test_reachable.contains(&module.name)
                && !self.production_reachable.contains(&module.name);
            if !from_test && !self.production_reachable.contains(&module.name) {
                continue;
            }
            for reference in &module.references {
                if let Some((target_module, symbol)) =
                    module.resolve_reference(&reference.name, &known, &declarations)
                {
                    output.insert(ResolvedReference {
                        target_module,
                        symbol,
                        source_module: module.name.clone(),
                        source_symbol: reference.owner.clone(),
                        path: module.path.clone(),
                        from_test,
                    });
                }
            }
        }
        output.into_iter().collect()
    }

    fn production_symbols(&self, references: &[ResolvedReference]) -> BTreeSet<(String, String)> {
        let declarations: BTreeSet<_> = self
            .declarations()
            .map(|(module, declaration)| (module.name.clone(), declaration.name.clone()))
            .collect();
        let mut reached: BTreeSet<_> = self
            .declarations()
            .filter(|(module, declaration)| {
                declaration.top_level && self.production_roots.contains(&module.name)
            })
            .map(|(module, declaration)| (module.name.clone(), declaration.name.clone()))
            .collect();
        for reference in references
            .iter()
            .filter(|reference| !reference.from_test && reference.source_symbol.is_none())
        {
            let target = (reference.target_module.clone(), reference.symbol.clone());
            if declarations.contains(&target) {
                reached.insert(target);
            }
        }

        loop {
            let before = reached.len();
            for reference in references.iter().filter(|reference| !reference.from_test) {
                let Some(source_symbol) = &reference.source_symbol else {
                    continue;
                };
                let source = (reference.source_module.clone(), source_symbol.clone());
                if !reached.contains(&source) {
                    continue;
                }
                let target = (reference.target_module.clone(), reference.symbol.clone());
                if declarations.contains(&target) {
                    reached.insert(target);
                }
            }
            if reached.len() == before {
                break;
            }
        }
        reached
    }

    fn private_couplings(&self) -> Vec<PrivateCoupling> {
        let packages = self.production_packages();
        let known: BTreeSet<_> = self.modules.keys().cloned().collect();
        let declarations = BTreeSet::new();
        let mut output = BTreeSet::new();
        for module in self.modules.values().filter(|module| module.is_test) {
            for coupling in &module.private_couplings {
                let target = module
                    .resolve_reference(&coupling.target, &known, &declarations)
                    .map_or_else(
                        || coupling.target.clone(),
                        |(name, symbol)| {
                            if symbol.is_empty() {
                                name
                            } else {
                                format!("{name}.{symbol}")
                            }
                        },
                    );
                if target
                    .split('.')
                    .next()
                    .is_some_and(|package| packages.contains(package))
                    && has_private_component(&target)
                {
                    output.insert(PrivateCoupling {
                        path: module.path.clone(),
                        line: coupling.line,
                        test_name: coupling.test_name.clone(),
                        kind: coupling.kind,
                        target,
                    });
                }
            }
        }
        output.into_iter().collect()
    }
}

fn ast_terminal(
    selected_files: u32,
    corpus: Corpus,
    index: usize,
    path: &str,
    work: &AstWork,
) -> (u32, Corpus, Terminal) {
    let terminal = match work.failure() {
        Some(AstFailure::Deadline) => Terminal::TimedOut {
            elapsed_ms: work.elapsed_ms(),
        },
        Some(AstFailure::Nodes | AstFailure::RetainedText) => Terminal::WorkExhausted {
            documents: u32::try_from(index).unwrap_or(u32::MAX),
            path: path.to_owned(),
            diagnostic: work.diagnostic(),
        },
        None => unreachable!("AST terminal requires one exhausted work bound"),
    };
    (selected_files, corpus, terminal)
}

fn graph_terminal(
    selected_files: u32,
    corpus: Corpus,
    work: &GraphWork,
) -> (u32, Corpus, Terminal) {
    let terminal = match work.failure() {
        Some(GraphFailure::Deadline) => Terminal::TimedOut {
            elapsed_ms: work.elapsed_ms(),
        },
        Some(_) => Terminal::WorkExhausted {
            documents: selected_files,
            path: "<graph>".to_owned(),
            diagnostic: work.diagnostic(),
        },
        None => unreachable!("graph terminal requires one exhausted work bound"),
    };
    (selected_files, corpus, terminal)
}

impl HygieneConfiguration {
    fn exception_matches(&self, coupling: &PrivateCoupling) -> bool {
        self.private_exceptions
            .iter()
            .any(|exception| exception.matches(coupling))
    }
}

impl PrivateCouplingException {
    fn matches(&self, coupling: &PrivateCoupling) -> bool {
        self.test_name == coupling.test_name && self.target == coupling.target
    }
}

fn reachable_bounded(
    roots: &BTreeSet<String>,
    edges: &BTreeSet<(String, String)>,
    work: &mut GraphWork,
) -> Option<BTreeSet<String>> {
    if roots.len() > GRAPH_WORKLIST_LIMIT {
        work.failure = Some(GraphFailure::Worklist);
        return None;
    }
    let mut reached = BTreeSet::new();
    let mut pending: Vec<_> = roots.iter().cloned().collect();
    while let Some(next) = pending.pop() {
        if !work.step() {
            return None;
        }
        if !reached.insert(next.clone()) {
            continue;
        }
        for (from, to) in edges {
            if !work.step() {
                return None;
            }
            if from == &next {
                if !work.admit_worklist(pending.len()) {
                    return None;
                }
                pending.push(to.clone());
            }
        }
    }
    Some(reached)
}

fn reachable(roots: &BTreeSet<String>, edges: &BTreeSet<(String, String)>) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<_> = roots.iter().cloned().collect();
    while let Some(next) = pending.pop() {
        if !reached.insert(next.clone()) {
            continue;
        }
        pending.extend(
            edges
                .iter()
                .filter(|(from, _)| from == &next)
                .map(|(_, to)| to.clone()),
        );
    }
    reached
}

fn finding_bytes(finding: &HygieneFinding) -> usize {
    let strings = match &finding.detail {
        HygieneFindingDetail::ModuleReachability {
            provider,
            subject,
            imported_by,
        } => provider
            .len()
            .saturating_add(subject.len())
            .saturating_add(imported_by.iter().map(String::len).sum()),
        HygieneFindingDetail::SymbolReachability {
            provider,
            module,
            symbol,
            symbol_kind,
            imported_by,
        } => provider
            .len()
            .saturating_add(module.len())
            .saturating_add(symbol.len())
            .saturating_add(symbol_kind.len())
            .saturating_add(imported_by.iter().map(String::len).sum()),
        HygieneFindingDetail::PythonUnused {
            module,
            symbol,
            declaration_kind,
            reason,
            test_references,
            ..
        } => module
            .len()
            .saturating_add(symbol.len())
            .saturating_add(declaration_kind.len())
            .saturating_add(reason.len())
            .saturating_add(test_references.iter().map(String::len).sum()),
        HygieneFindingDetail::AssertionFree { test_name } => test_name.len(),
        HygieneFindingDetail::PrivateCoupling {
            test_name,
            coupling_kind,
            target,
        } => test_name
            .len()
            .saturating_add(coupling_kind.len())
            .saturating_add(target.len()),
        HygieneFindingDetail::StalePrivateException { test_name, target } => {
            test_name.len().saturating_add(target.len())
        }
    };
    finding.path.len().saturating_add(strings)
}
