#[derive(Default)]
struct Corpus {
    modules: BTreeMap<String, Module>,
    edges: BTreeSet<(String, String)>,
    production_roots: BTreeSet<String>,
    test_modules: BTreeSet<String>,
    production_reachable: BTreeSet<String>,
    test_reachable: BTreeSet<String>,
}

impl Corpus {
    fn load(
        jurisdiction: &Jurisdiction,
        configuration: &HygieneConfiguration,
        started: Instant,
    ) -> (u32, Self, Terminal) {
        let paths: Vec<_> = jurisdiction
            .measurable()
            .filter(|entry| is_python_path(entry.path()))
            .map(|entry| entry.path().to_owned())
            .collect();
        let selected_files = u32::try_from(paths.len()).unwrap_or(u32::MAX);
        let mut corpus = Self::default();
        for (index, path) in paths.iter().enumerate() {
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if elapsed_ms > configuration.deadline_ms {
                return (selected_files, corpus, Terminal::TimedOut { elapsed_ms });
            }
            let absolute = jurisdiction.root().join(path);
            let bytes = match fs::read(&absolute) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (
                        selected_files,
                        corpus,
                        Terminal::Failed {
                            stage: "selected-tree-read",
                        },
                    );
                }
            };
            let source = match std::str::from_utf8(&bytes) {
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
            let mut module = Module::new(path.clone(), name.clone(), is_test, source);
            {
                let mut collector = AstCollector::new(&mut module, configuration);
                for statement in suite {
                    collector.visit_stmt(statement);
                }
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
        corpus.construct_graphs(configuration);
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let terminal = if elapsed_ms > configuration.deadline_ms {
            Terminal::TimedOut { elapsed_ms }
        } else {
            Terminal::Parsed
        };
        (selected_files, corpus, terminal)
    }

    fn construct_graphs(&mut self, configuration: &HygieneConfiguration) {
        let known: BTreeSet<_> = self.modules.keys().cloned().collect();
        for (name, module) in &self.modules {
            if module.is_test {
                self.test_modules.insert(name.clone());
            }
            if !module.is_test && path_is_under_roots(&module.path, &configuration.production_roots)
            {
                self.production_roots.insert(name.clone());
            }
            for import in &module.imports {
                if let Some(target) = import.target_module(&known) {
                    self.edges.insert((name.clone(), target));
                }
            }
        }
        self.production_reachable = reachable(&self.production_roots, &self.edges);
        self.test_reachable = reachable(&self.test_modules, &self.edges);
    }

    fn applicable(&self, gate: GateId) -> bool {
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

    fn traversal_evidence(
        &self,
        traversal: HygieneTraversal,
        configuration: &HygieneConfiguration,
    ) -> AnalysisEvidence {
        match traversal {
            HygieneTraversal::ImportGraph => AnalysisEvidence::GraphConstruction {
                production_roots: self.root_paths(&self.production_roots),
                test_roots: self.root_paths(&self.test_modules),
                nodes: u32::try_from(self.modules.len()).unwrap_or(u32::MAX),
                edges: u32::try_from(self.edges.len()).unwrap_or(u32::MAX),
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

    fn findings(&self, gate: GateId, configuration: &HygieneConfiguration) -> Vec<HygieneFinding> {
        match gate {
            GateId::Reachability => self.reachability_findings(),
            GateId::SymbolReachability => self.symbol_reachability_findings(),
            GateId::PythonUnused => self.python_unused_findings(),
            GateId::AssertionFreeTests => self.assertion_free_findings(),
            GateId::PrivateInternals => self.private_findings(configuration),
            _ => Vec::new(),
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

    fn private_findings(&self, configuration: &HygieneConfiguration) -> Vec<HygieneFinding> {
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
                .filter(|exception| !couplings.iter().any(|item| exception.matches(item)))
                .map(|exception| HygieneFinding {
                    path: "spice.toml".to_owned(),
                    line: None,
                    detail: HygieneFindingDetail::StalePrivateException {
                        test_name: exception.test_name.clone(),
                        target: exception.target.clone(),
                    },
                }),
        );
        findings
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

    fn resolved_references(&self) -> Vec<ResolvedReference> {
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
