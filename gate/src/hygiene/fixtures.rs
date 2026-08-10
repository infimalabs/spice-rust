use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration_for(gate: GateId) -> HygieneDeclaration {
        HYGIENE_ARMS
            .into_iter()
            .find(|arm| arm.id == gate)
            .expect("fixture gate belongs to hygiene family")
    }

    fn language(applicable: bool) -> AnalysisEvidence {
        AnalysisEvidence::LanguageApplicability {
            language: "python".to_owned(),
            selected_files: u32::from(applicable) * 6,
            project_markers: if applicable {
                vec!["spice.toml".to_owned(), "pyproject.toml".to_owned()]
            } else {
                Vec::new()
            },
            applicable,
            provenance: HygieneProvenance {
                release: "current".to_owned(),
                layer: "compiled".to_owned(),
                document: "<compiled-gate-battery>".to_owned(),
                field: "gate::hygiene::HygieneJurisdiction::applicability".to_owned(),
            },
        }
    }

    fn native_analyzer(gate: GateId) -> AnalysisEvidence {
        let arm = declaration_for(gate);
        AnalysisEvidence::Analyzer(AnalyzerEvidence::Identified {
            name: arm.analyzer.evidence_name().to_owned(),
            requested: arm.analyzer.component().to_owned(),
            resolved: format!("compiled:hermetic:{}.rlib", arm.analyzer.evidence_name()),
            declared_version: NATIVE_ANALYZER_VERSION.to_owned(),
            observed_version: NATIVE_ANALYZER_VERSION.to_owned(),
            digest: format!("sha256:{:02x}", gate as u8),
        })
    }

    fn graph() -> AnalysisEvidence {
        AnalysisEvidence::GraphConstruction {
            production_roots: vec![
                "spice/cli/entry.py".to_owned(),
                "spice/hooks/precommit.py".to_owned(),
            ],
            test_roots: vec![
                "tests/test_cli.py".to_owned(),
                "tests/test_hooks.py".to_owned(),
            ],
            nodes: 9,
            edges: 14,
            // The two counts differ on the entry module and agree on the hook,
            // so a fixture reading this carries the gap the measurement is for
            // rather than one number repeated under two names.
            reach: vec![
                ModuleReach {
                    path: "spice/cli/entry.py".to_owned(),
                    reached_by: 2,
                    imported_by: 1,
                },
                ModuleReach {
                    path: "spice/hooks/precommit.py".to_owned(),
                    reached_by: 1,
                    imported_by: 1,
                },
            ],
        }
    }

    fn symbols() -> AnalysisEvidence {
        AnalysisEvidence::SymbolExtraction {
            declarations: 7,
            production_references: 4,
            test_references: 3,
        }
    }

    fn tests() -> AnalysisEvidence {
        AnalysisEvidence::TestClassification {
            files: 2,
            tests: 4,
            assertions: 3,
            helpers: vec!["expect_error".to_owned(), "assert_json".to_owned()],
        }
    }

    fn boundaries() -> AnalysisEvidence {
        AnalysisEvidence::PrivateBoundaryResolution {
            packages: vec!["spice".to_owned()],
            private_imports: 1,
            private_assertions: 1,
            admitted_exceptions: 0,
            stale_exceptions: 0,
        }
    }

    fn completed_evidence(gate: GateId, observed: u32) -> Vec<AnalysisEvidence> {
        let field = match gate {
            GateId::Reachability => "policy.debt.reachability_test_only",
            GateId::AssertionFreeTests => "policy.debt.assertion_free_tests",
            _ => "compiled:zero-findings",
        };
        let mut evidence = vec![
            language(true),
            native_analyzer(gate),
            AnalysisEvidence::PolicyDecision {
                field: field.to_owned(),
                limit: 0,
                observed,
            },
            AnalysisEvidence::Deadline {
                limit_ms: 30_000,
                elapsed_ms: 17,
                state: EvidenceState::Completed,
            },
            AnalysisEvidence::Parse {
                documents: 6,
                state: EvidenceState::Completed,
                failed_path: None,
                diagnostic: None,
            },
            AnalysisEvidence::Measurement {
                state: EvidenceState::Completed,
                stage: "policy-decision".to_owned(),
            },
        ];
        for traversal in declaration_for(gate).traversals {
            let item = match traversal {
                HygieneTraversal::ImportGraph => graph(),
                HygieneTraversal::SymbolGraph | HygieneTraversal::PythonDeclarations => symbols(),
                HygieneTraversal::TestAssertions => tests(),
                HygieneTraversal::PrivateBoundaries => boundaries(),
            };
            if !evidence
                .iter()
                .any(|existing| evidence_rank(existing) == evidence_rank(&item))
            {
                evidence.push(item);
            }
        }
        evidence
    }

    fn finding(gate: GateId) -> HygieneFinding {
        match gate {
            GateId::Reachability => HygieneFinding {
                path: "spice/test_support.py".to_owned(),
                line: None,
                detail: HygieneFindingDetail::ModuleReachability {
                    provider: "python".to_owned(),
                    subject: "spice.test_support".to_owned(),
                    imported_by: vec!["test_cli.py".to_owned(), "test_hooks.py".to_owned()],
                },
            },
            GateId::SymbolReachability => HygieneFinding {
                path: "spice/core.py".to_owned(),
                line: Some(23),
                detail: HygieneFindingDetail::SymbolReachability {
                    provider: "python".to_owned(),
                    module: "spice.core".to_owned(),
                    symbol: "TestOnly.method".to_owned(),
                    symbol_kind: "method".to_owned(),
                    imported_by: vec!["test_core.py".to_owned()],
                },
            },
            GateId::PythonUnused => HygieneFinding {
                path: "spice/unused.py".to_owned(),
                line: Some(11),
                detail: HygieneFindingDetail::PythonUnused {
                    module: "spice.unused".to_owned(),
                    symbol: "orphan".to_owned(),
                    declaration_kind: "function".to_owned(),
                    status: PythonUnusedStatus::CandidateUnused,
                    reason: "no production or test references".to_owned(),
                    test_references: Vec::new(),
                },
            },
            GateId::AssertionFreeTests => HygieneFinding {
                path: "tests/test_quality.py".to_owned(),
                line: Some(19),
                detail: HygieneFindingDetail::AssertionFree {
                    test_name: "TestQuality.test_runs".to_owned(),
                },
            },
            GateId::PrivateInternals => HygieneFinding {
                path: "tests/test_private.py".to_owned(),
                line: Some(4),
                detail: HygieneFindingDetail::PrivateCoupling {
                    test_name: "<module>".to_owned(),
                    coupling_kind: "private import".to_owned(),
                    target: "spice._internal".to_owned(),
                },
            },
            _ => panic!("fixture requested a foreign gate"),
        }
    }

    fn predecessor_finding(gate: GateId) -> HygieneFinding {
        match gate {
            GateId::Reachability => HygieneFinding {
                path: "spice/test_support.py".to_owned(),
                line: None,
                detail: HygieneFindingDetail::ModuleReachability {
                    provider: "python".to_owned(),
                    subject: "spice.test_support".to_owned(),
                    imported_by: vec!["test_cli.py".to_owned(), "test_hooks.py".to_owned()],
                },
            },
            GateId::SymbolReachability => HygieneFinding {
                path: "spice/core.py".to_owned(),
                line: Some(23),
                detail: HygieneFindingDetail::SymbolReachability {
                    provider: "python".to_owned(),
                    module: "spice.core".to_owned(),
                    symbol: "TestOnly.method".to_owned(),
                    symbol_kind: "method".to_owned(),
                    imported_by: vec!["test_core.py".to_owned()],
                },
            },
            GateId::PythonUnused => HygieneFinding {
                path: "spice/unused.py".to_owned(),
                line: Some(11),
                detail: HygieneFindingDetail::PythonUnused {
                    module: "spice.unused".to_owned(),
                    symbol: "orphan".to_owned(),
                    declaration_kind: "function".to_owned(),
                    status: PythonUnusedStatus::CandidateUnused,
                    reason: "no production or test references".to_owned(),
                    test_references: Vec::new(),
                },
            },
            GateId::AssertionFreeTests => HygieneFinding {
                path: "tests/test_quality.py".to_owned(),
                line: Some(19),
                detail: HygieneFindingDetail::AssertionFree {
                    test_name: "TestQuality.test_runs".to_owned(),
                },
            },
            GateId::PrivateInternals => HygieneFinding {
                path: "tests/test_private.py".to_owned(),
                line: Some(4),
                detail: HygieneFindingDetail::PrivateCoupling {
                    test_name: "<module>".to_owned(),
                    coupling_kind: "private import".to_owned(),
                    target: "spice._internal".to_owned(),
                },
            },
            _ => panic!("fixture requested a foreign predecessor gate"),
        }
    }

    fn observation(gate: GateId, outcome: HygieneOutcome) -> HygieneObservation {
        let findings = if outcome == HygieneOutcome::Findings {
            vec![finding(gate)]
        } else {
            Vec::new()
        };
        let evidence = match outcome {
            HygieneOutcome::Passed | HygieneOutcome::Findings => {
                completed_evidence(gate, findings.len() as u32)
            }
            HygieneOutcome::NotApplicable => vec![language(false)],
            HygieneOutcome::Excluded => {
                vec![AnalysisEvidence::Exclusion(ExclusionEvidence {
                    provenance: HygieneProvenance {
                        release: "current".to_owned(),
                        layer: "repository".to_owned(),
                        document: "spice.toml".to_owned(),
                        field: "policy.exclude[0]".to_owned(),
                    },
                    reason: "generated fixture tree".to_owned(),
                })]
            }
            HygieneOutcome::Unavailable => {
                vec![AnalysisEvidence::Analyzer(AnalyzerEvidence::Unavailable {
                    name: declaration_for(gate).analyzer.evidence_name().to_owned(),
                    requested: declaration_for(gate).analyzer.component().to_owned(),
                })]
            }
            HygieneOutcome::Unsupported => {
                let arm = declaration_for(gate);
                vec![AnalysisEvidence::Analyzer(AnalyzerEvidence::Identified {
                    name: arm.analyzer.evidence_name().to_owned(),
                    requested: arm.analyzer.component().to_owned(),
                    resolved: "compiled:fixture".to_owned(),
                    declared_version: NATIVE_ANALYZER_VERSION.to_owned(),
                    observed_version: "spice-gate-hygiene-v0".to_owned(),
                    digest: "sha256:old".to_owned(),
                })]
            }
            HygieneOutcome::Unmeasured => {
                vec![AnalysisEvidence::Measurement {
                    state: EvidenceState::Unmeasured,
                    stage: "scheduler".to_owned(),
                }]
            }
            HygieneOutcome::Timeout => vec![AnalysisEvidence::Deadline {
                limit_ms: 30_000,
                elapsed_ms: 30_001,
                state: EvidenceState::TimedOut,
            }],
            HygieneOutcome::ParseFailure => {
                vec![AnalysisEvidence::Parse {
                    documents: 2,
                    state: EvidenceState::ParseFailed,
                    failed_path: Some("spice/broken.py".to_owned()),
                    diagnostic: Some("unexpected indent at line 7".to_owned()),
                }]
            }
            HygieneOutcome::Failed => {
                vec![AnalysisEvidence::Measurement {
                    state: EvidenceState::Failed,
                    stage: "private-boundary-resolution".to_owned(),
                }]
            }
        };
        HygieneObservation {
            gate_id: gate,
            outcome,
            reason: format!("{} fixture", gate.as_str()),
            evidence,
            findings,
        }
    }

    fn report_with(target: GateId, outcome: HygieneOutcome) -> HygieneReport {
        measure_hygiene(HYGIENE_ARMS.into_iter().map(|arm| {
            observation(
                arm.id,
                if arm.id == target {
                    outcome
                } else {
                    HygieneOutcome::Passed
                },
            )
        }))
        .expect("fixture observations satisfy declarations")
    }

    /// A reach ranking out of its own order is refused, and so is a zero rank.
    ///
    /// Nothing downstream re-sorts this ranking, because sorting it by path
    /// would discard the one thing it says. That leaves the order load-bearing
    /// on arrival: a ranking whose narrowest reach comes first names the wrong
    /// module as the one a lane cannot verify alone. A module no test reaches
    /// is left out rather than ranked at zero, so a zero row is the same kind
    /// of claim about a walk that never happened.
    #[test]
    fn graph_evidence_ranked_against_its_own_order_or_at_zero_is_refused() {
        for (label, edit) in [
            (
                "narrowest first",
                (|reach| reach.reverse()) as fn(&mut Vec<ModuleReach>),
            ),
            (
                "tied rows out of path order",
                |reach: &mut Vec<ModuleReach>| {
                    reach[0].reached_by = reach[1].reached_by;
                    reach[0].imported_by = reach[1].imported_by;
                    reach.swap(0, 1);
                },
            ),
            ("reached by nothing", |reach: &mut Vec<ModuleReach>| {
                reach[1].reached_by = 0;
                reach[1].imported_by = 0;
            }),
        ] {
            let forged = HYGIENE_ARMS.into_iter().map(|arm| {
                let mut observed = observation(arm.id, HygieneOutcome::Passed);
                if arm.id == GateId::Reachability {
                    for evidence in &mut observed.evidence {
                        if let AnalysisEvidence::GraphConstruction { reach, .. } = evidence {
                            edit(reach);
                        }
                    }
                }
                observed
            });

            let Err(error) = measure_hygiene(forged) else {
                panic!("a ranking {label} must be refused")
            };
            assert!(
                matches!(
                    &error,
                    HygieneError::InvalidEvidence { gate, evidence }
                        if *gate == GateId::Reachability
                            && evidence == "malformed typed stage evidence"
                ),
                "{label}: unexpected refusal: {error}"
            );
        }
    }

    /// More importers than reachers is two graphs published as one.
    ///
    /// A test module naming a module directly also reaches it, so the direct
    /// count is the narrower of the two by construction. Evidence where it is
    /// wider was never measured from one traversal, and a report carrying it
    /// would rank a module by a reach nothing walked.
    #[test]
    fn graph_evidence_counting_more_importers_than_reachers_is_refused() {
        let forged = HYGIENE_ARMS.into_iter().map(|arm| {
            let mut observed = observation(arm.id, HygieneOutcome::Passed);
            if arm.id == GateId::Reachability {
                for evidence in &mut observed.evidence {
                    if let AnalysisEvidence::GraphConstruction { reach, .. } = evidence {
                        reach[0].imported_by = reach[0].reached_by + 1;
                    }
                }
            }
            observed
        });

        let Err(error) = measure_hygiene(forged) else {
            panic!("evidence counting more importers than reachers must be refused")
        };
        assert!(
            matches!(
                &error,
                HygieneError::InvalidEvidence { gate, evidence }
                    if *gate == GateId::Reachability
                        && evidence == "malformed typed stage evidence"
            ),
            "unexpected refusal: {error}"
        );
    }

    #[test]
    fn declarations_are_the_exact_final_battery_interval() {
        let ids: Vec<_> = HYGIENE_ARMS.iter().map(|arm| arm.id()).collect();
        assert_eq!(ids, GateId::BATTERY[22..]);
        for arm in HYGIENE_ARMS {
            assert!(
                arm.native_component()
                    .starts_with("gate::hygiene::HygieneAnalyzer::")
            );
            assert!(!arm.analyzer().evidence_name().is_empty());
            assert!(!arm.jurisdiction().roots().is_empty());
            assert!(!arm.jurisdiction().includes().is_empty());
            assert!(!arm.jurisdiction().excludes().is_empty());
            assert!(!arm.jurisdiction().applicability().is_empty());
            assert!(!arm.configuration().is_empty());
            assert!(!arm.traversals().is_empty());
            for source in arm.configuration() {
                assert!(!source.source().is_empty());
                assert!(!source.field().is_empty());
            }
        }
        for arm in &HYGIENE_ARMS[..2] {
            assert_eq!(arm.provider_contract(), Some(EXACT_PROVIDER_CONTRACT));
        }
        for arm in &HYGIENE_ARMS[2..] {
            assert_eq!(arm.provider_contract(), None);
        }
    }

    #[test]
    fn pass_and_finding_fixtures_retain_every_analysis_stage() {
        for arm in HYGIENE_ARMS {
            for outcome in [HygieneOutcome::Passed, HygieneOutcome::Findings] {
                let report = report_with(arm.id, outcome);
                let measured = report
                    .arms
                    .iter()
                    .find(|item| item.gate_id == arm.id)
                    .expect("target measurement");
                assert_eq!(measured.outcome, outcome);
                assert_eq!(
                    measured.outcome.satisfies_gate(),
                    outcome == HygieneOutcome::Passed
                );
                assert_eq!(
                    measured.findings.is_empty(),
                    outcome == HygieneOutcome::Passed
                );
                assert!(has_applicable_language(&measured.evidence));
                assert!(has_native_analyzer(arm, &measured.evidence));
                assert!(has_completed_deadline(&measured.evidence));
                assert!(has_completed_parse(&measured.evidence));
                for traversal in arm.traversals {
                    assert!(has_traversal_evidence(*traversal, &measured.evidence));
                }
            }
        }
    }

    #[test]
    fn graph_symbol_and_policy_fixture_identities_are_multifile() {
        let reachability = report_with(GateId::Reachability, HygieneOutcome::Findings);
        let arm = &reachability.arms[0];
        let graph = arm.evidence.iter().find_map(|item| match item {
            AnalysisEvidence::GraphConstruction {
                production_roots,
                test_roots,
                nodes,
                edges,
                reach,
            } => Some((production_roots, test_roots, nodes, edges, reach)),
            _ => None,
        });
        let Some((production, tests, nodes, edges, reach)) = graph else {
            panic!("graph evidence retained");
        };
        assert_eq!(production.len(), 2);
        assert_eq!(tests.len(), 2);
        assert_eq!((*nodes, *edges), (9, 14));
        assert_eq!(
            reach
                .iter()
                .map(|entry| (entry.reached_by, entry.imported_by))
                .collect::<Vec<_>>(),
            [(2, 1), (1, 1)],
            "graph evidence retains the reach ranking widest first"
        );

        for gate in GateId::BATTERY[22..].iter().copied() {
            let report = report_with(gate, HygieneOutcome::Findings);
            let measured = report
                .arms
                .iter()
                .find(|item| item.gate_id == gate)
                .expect("finding measurement");
            assert_eq!(measured.findings, vec![finding(gate)]);
        }

        let stale = HygieneFinding {
            path: "spice.toml".to_owned(),
            line: None,
            detail: HygieneFindingDetail::StalePrivateException {
                test_name: "test_private".to_owned(),
                target: "spice._gone".to_owned(),
            },
        };
        assert_eq!(stale.detail.gate(), GateId::PrivateInternals);
    }

    #[test]
    fn every_nonpass_state_is_typed_for_every_arm_and_never_satisfies() {
        let outcomes = [
            HygieneOutcome::Findings,
            HygieneOutcome::NotApplicable,
            HygieneOutcome::Excluded,
            HygieneOutcome::Unavailable,
            HygieneOutcome::Unsupported,
            HygieneOutcome::Unmeasured,
            HygieneOutcome::Timeout,
            HygieneOutcome::ParseFailure,
            HygieneOutcome::Failed,
        ];
        for arm in HYGIENE_ARMS {
            for outcome in outcomes {
                let report = report_with(arm.id, outcome);
                let measured = report
                    .arms
                    .iter()
                    .find(|item| item.gate_id == arm.id)
                    .expect("target state");
                assert_eq!(measured.outcome, outcome);
                assert_eq!(
                    measured.outcome.satisfies_gate(),
                    matches!(
                        outcome,
                        HygieneOutcome::NotApplicable | HygieneOutcome::Excluded
                    )
                );
                assert_eq!(
                    measured.outcome.is_skip(),
                    matches!(
                        outcome,
                        HygieneOutcome::NotApplicable | HygieneOutcome::Excluded
                    )
                );
                assert!(!measured.reason.is_empty());
                assert!(!measured.configuration.is_empty());
                assert!(!measured.evidence.is_empty());
            }
        }
    }

    #[test]
    fn reports_are_canonical_across_observation_and_evidence_order() {
        let ordered: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Findings))
            .collect();
        let first = measure_hygiene(ordered.clone()).expect("ordered report");
        let mut reversed = ordered;
        reversed.reverse();
        for observation in &mut reversed {
            observation.evidence.reverse();
            observation.findings.reverse();
        }
        let second = measure_hygiene(reversed).expect("reversed report");
        assert_eq!(first, second);
        assert_eq!(first.version, HYGIENE_REPORT_VERSION);
        assert_eq!(first.arms.len(), HYGIENE_ARMS.len());
        assert_eq!(first.canonical_json(), second.canonical_json());
        assert_eq!(first.canonical_json().last(), Some(&b'\n'));
    }

    #[test]
    fn inconsistent_or_incomplete_states_are_refused() {
        let duplicate = GateId::Reachability;
        assert_eq!(
            measure_hygiene([
                observation(duplicate, HygieneOutcome::Passed),
                observation(duplicate, HygieneOutcome::Passed),
            ]),
            Err(HygieneError::DuplicateGate(duplicate))
        );
        assert_eq!(
            measure_hygiene(std::iter::empty()),
            Err(HygieneError::MissingGate(GateId::Reachability))
        );

        let mut observations: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Passed))
            .collect();
        observations[0]
            .evidence
            .retain(|item| !matches!(item, AnalysisEvidence::GraphConstruction { .. }));
        assert!(matches!(
            measure_hygiene(observations),
            Err(HygieneError::InvalidEvidence {
                gate: GateId::Reachability,
                ..
            })
        ));

        let mut bad_findings: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Passed))
            .collect();
        bad_findings[1].outcome = HygieneOutcome::Findings;
        bad_findings[1].findings = vec![finding(GateId::Reachability)];
        assert!(matches!(
            measure_hygiene(bad_findings),
            Err(HygieneError::InvalidFindings {
                gate: GateId::SymbolReachability,
                ..
            })
        ));

        let mut unresolved: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Passed))
            .collect();
        let analyzer = unresolved[0]
            .evidence
            .iter_mut()
            .find_map(|item| match item {
                AnalysisEvidence::Analyzer(AnalyzerEvidence::Identified { resolved, .. }) => {
                    Some(resolved)
                }
                _ => None,
            });
        analyzer.expect("native analyzer evidence").clear();
        assert!(matches!(
            measure_hygiene(unresolved),
            Err(HygieneError::InvalidEvidence {
                gate: GateId::Reachability,
                ..
            })
        ));

        for suppressed in [Vec::new(), vec![unattributable_suppression()]] {
            let mut forged: Vec<_> = HYGIENE_ARMS
                .into_iter()
                .map(|arm| observation(arm.id, HygieneOutcome::Passed))
                .collect();
            forged[0]
                .evidence
                .push(AnalysisEvidence::PathSuppression { suppressed });
            assert!(
                matches!(
                    measure_hygiene(forged),
                    Err(HygieneError::InvalidEvidence {
                        gate: GateId::Reachability,
                        ..
                    })
                ),
                "a suppression a reader cannot trace back to a rule is not evidence"
            );
        }

        let mut unordered: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Passed))
            .collect();
        let late = suppression("tests/test_ignored.py");
        let early = suppression("app/_ignored.py");
        unordered[0]
            .evidence
            .push(AnalysisEvidence::PathSuppression {
                suppressed: vec![late.clone(), early.clone(), late.clone()],
            });
        let measured = measure_hygiene(unordered).expect("complete suppressions are admissible");
        assert_eq!(
            measured.arms[0]
                .evidence
                .iter()
                .find_map(|item| match item {
                    AnalysisEvidence::PathSuppression { suppressed } => Some(suppressed.as_slice()),
                    _ => None,
                }),
            Some([early, late].as_slice()),
            "retained suppressions are one canonical set however they arrived"
        );
    }

    /// A suppression naming a path but no rule that removed it.
    fn unattributable_suppression() -> SuppressedPath {
        SuppressedPath {
            rule: String::new(),
            ..suppression("tests/test_ignored.py")
        }
    }

    fn suppression(path: &str) -> SuppressedPath {
        SuppressedPath {
            path: path.to_owned(),
            shape: "file".to_owned(),
            rule: path.to_owned(),
            provenance: HygieneProvenance {
                release: "current".to_owned(),
                layer: "repository".to_owned(),
                document: "spice.toml".to_owned(),
                field: "gate.walk.skip[0]".to_owned(),
            },
        }
    }

    #[test]
    fn external_provider_findings_require_exact_identity_evidence() {
        let gate = GateId::Reachability;
        let mut observations: Vec<_> = HYGIENE_ARMS
            .into_iter()
            .map(|arm| observation(arm.id, HygieneOutcome::Passed))
            .collect();
        {
            let target = observations
                .iter_mut()
                .find(|item| item.gate_id == gate)
                .expect("reachability observation");
            target.outcome = HygieneOutcome::Findings;
            target.findings = vec![HygieneFinding {
                path: "src/lib.rs".to_owned(),
                line: None,
                detail: HygieneFindingDetail::ModuleReachability {
                    provider: "rust-provider".to_owned(),
                    subject: "crate::unused".to_owned(),
                    imported_by: vec!["tests/reach.rs".to_owned()],
                },
            }];
            let decision = target.evidence.iter_mut().find_map(|item| match item {
                AnalysisEvidence::PolicyDecision { observed, .. } => Some(observed),
                _ => None,
            });
            *decision.expect("provider policy decision") = 1;
        }
        assert!(matches!(
            measure_hygiene(observations.clone()),
            Err(HygieneError::InvalidEvidence { gate: observed, .. })
                if observed == gate
        ));

        let target = observations
            .iter_mut()
            .find(|item| item.gate_id == gate)
            .expect("reachability observation");
        target
            .evidence
            .push(AnalysisEvidence::Analyzer(AnalyzerEvidence::Identified {
                name: "rust-provider".to_owned(),
                requested: "reach-rs".to_owned(),
                resolved: "/tools/reach-rs".to_owned(),
                declared_version: "reach-rs 2.4.0".to_owned(),
                observed_version: "reach-rs 2.4.0".to_owned(),
                digest: "sha256:provider".to_owned(),
            }));
        let report = measure_hygiene(observations).expect("exact provider identity is admissible");
        assert!(report.arms[0].has_external_provider());
    }

    #[test]
    fn predecessor_builtins_are_equivalent_and_old_providers_refuse() {
        for gate in GateId::BATTERY[22..].iter().copied() {
            for outcome in [HygieneOutcome::Passed, HygieneOutcome::Findings] {
                let report = report_with(gate, outcome);
                let current = report
                    .arms
                    .iter()
                    .find(|item| item.gate_id == gate)
                    .expect("current measurement");
                let predecessor = PredecessorObservation {
                    gate_name: gate.as_str().to_owned(),
                    result: if outcome == HygieneOutcome::Passed {
                        PredecessorResult::Passed
                    } else {
                        PredecessorResult::Findings(vec![predecessor_finding(gate)])
                    },
                };
                assert_eq!(
                    compare_predecessor(&predecessor, current),
                    Ok(PredecessorComparison::Equivalent)
                );
            }
        }

        for gate in [GateId::Reachability, GateId::AssertionFreeTests] {
            let mut observations: Vec<_> = HYGIENE_ARMS
                .into_iter()
                .map(|arm| observation(arm.id, HygieneOutcome::Passed))
                .collect();
            let target = observations
                .iter_mut()
                .find(|item| item.gate_id == gate)
                .expect("configured debt fixture");
            target.findings = vec![finding(gate)];
            let decision = target.evidence.iter_mut().find_map(|item| match item {
                AnalysisEvidence::PolicyDecision {
                    limit, observed, ..
                } => Some((limit, observed)),
                _ => None,
            });
            let Some((limit, observed)) = decision else {
                panic!("policy decision evidence");
            };
            *limit = 1;
            *observed = 1;
            let report =
                measure_hygiene(observations).expect("configured debt admits one retained finding");
            let current = report
                .arms
                .iter()
                .find(|item| item.gate_id == gate)
                .expect("debt measurement");
            let predecessor = PredecessorObservation {
                gate_name: gate.as_str().to_owned(),
                result: PredecessorResult::Passed,
            };
            assert_eq!(
                compare_predecessor(&predecessor, current),
                Ok(PredecessorComparison::Equivalent)
            );
            assert_eq!(current.findings, vec![finding(gate)]);
        }

        let current = report_with(GateId::Reachability, HygieneOutcome::Findings);
        let mut provider_finding = predecessor_finding(GateId::Reachability);
        let HygieneFindingDetail::ModuleReachability { provider, .. } =
            &mut provider_finding.detail
        else {
            panic!("reachability fixture type");
        };
        *provider = "custom-command".to_owned();
        let predecessor = PredecessorObservation {
            gate_name: "reachability".to_owned(),
            result: PredecessorResult::Findings(vec![provider_finding]),
        };
        let comparison = compare_predecessor(&predecessor, &current.arms[0])
            .expect("provider semantics have an explicit refusal");
        let PredecessorComparison::Refused(refusal) = comparison else {
            panic!("old provider evidence must refuse");
        };
        assert_eq!(refusal.release, PREDECESSOR_GATE_RELEASE);
        assert_eq!(refusal.gate_id, GateId::Reachability);
        assert_eq!(refusal.source_field, PREDECESSOR_PROVIDER_FIELD);
        assert!(!refusal.reason.is_empty());
        assert!(!refusal.recovery.is_empty());
    }

    #[test]
    fn predecessor_nondecision_states_have_release_owned_recovery() {
        let states = [
            HygieneOutcome::Unavailable,
            HygieneOutcome::Unsupported,
            HygieneOutcome::Unmeasured,
            HygieneOutcome::Timeout,
            HygieneOutcome::ParseFailure,
            HygieneOutcome::Failed,
        ];
        for gate in GateId::BATTERY[22..].iter().copied() {
            for outcome in states {
                let report = report_with(gate, outcome);
                let current = report
                    .arms
                    .iter()
                    .find(|item| item.gate_id == gate)
                    .expect("typed nondecision state");
                let predecessor = PredecessorObservation {
                    gate_name: gate.as_str().to_owned(),
                    result: PredecessorResult::Passed,
                };
                let comparison = compare_predecessor(&predecessor, current)
                    .expect("nondecision state has a refusal");
                let PredecessorComparison::Refused(refusal) = comparison else {
                    panic!("nondecision state must not migrate as a pass");
                };
                assert_eq!(refusal.release, PREDECESSOR_GATE_RELEASE);
                assert_eq!(refusal.gate_id, gate);
                assert_eq!(refusal.source_field, predecessor_arm_field(gate));
                assert!(!refusal.reason.is_empty());
                assert!(!refusal.recovery.is_empty());
            }
        }

        for outcome in [HygieneOutcome::NotApplicable, HygieneOutcome::Excluded] {
            let report = report_with(GateId::Reachability, outcome);
            let predecessor = PredecessorObservation {
                gate_name: "reachability".to_owned(),
                result: PredecessorResult::Passed,
            };
            assert_eq!(
                compare_predecessor(&predecessor, &report.arms[0]),
                Ok(PredecessorComparison::Equivalent)
            );
        }
    }
}
