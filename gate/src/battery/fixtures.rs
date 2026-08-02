use wire::Digest;

use super::ArmEvidence;
use crate::core::{
    CORE_GATE_IDS, ConfigInventoryEvidence, ConfigKeyEvidence, CoreArmInput, CoreEvidence,
    CoreFinding, CoreInputs, CoreObservation, CoreProvenance, DocMeasurement,
    MergeIntegrityEvidence, PlanPhaseEvidence, RepoDocsEvidence, RepoShapeEvidence,
    StagingEvidence, core_declarations, evaluate_core,
};
use crate::hygiene::{
    AnalysisEvidence, AnalyzerEvidence, EvidenceState as HygieneStage, ExclusionEvidence,
    HYGIENE_ARMS, HygieneObservation, HygieneOutcome, HygieneProvenance, HygieneTraversal,
    measure_hygiene,
};
use crate::manifest::GateId;
use crate::metrics::{
    EvidenceState as MetricState, LatchBinding, MetricEvidence, MetricId, MetricKey,
    MetricMeasurement, ParserEvidence, ParserId, RuleBinding, STRUCTURAL_ARMS,
    StructuralArmEvidence,
};
use crate::toolchain::{
    ArmObservation, ArmOutcome, TOOLCHAIN_ARMS, ToolEvidence, measure_toolchain,
};

pub(super) fn passing(source: &Digest) -> Vec<ArmEvidence> {
    evidence(source, false)
}

pub(super) fn nonpassing(source: &Digest) -> Vec<ArmEvidence> {
    evidence(source, true)
}

fn evidence(source: &Digest, nonpassing: bool) -> Vec<ArmEvidence> {
    let mut evidence = core(source, nonpassing);
    evidence.extend(toolchain(nonpassing));
    evidence.extend(metrics(nonpassing));
    evidence.extend(hygiene(nonpassing));
    assert_eq!(
        evidence
            .iter()
            .map(|item| item.gate_id().expect("compiled fixture"))
            .collect::<Vec<_>>(),
        GateId::ALL
    );
    evidence
}

fn core(source: &Digest, nonpassing: bool) -> Vec<ArmEvidence> {
    let arms = CORE_GATE_IDS.into_iter().map(|id| {
        let observation = if nonpassing {
            match id {
                GateId::MergeIntegrity => CoreObservation::Unavailable {
                    reason: "git executable was not admitted".to_owned(),
                },
                GateId::PlanPhase => CoreObservation::Skipped {
                    reason: "no claim store is configured for this repository".to_owned(),
                },
                GateId::RepoShape => CoreObservation::Measured {
                    evidence: CoreEvidence::RepoShape(RepoShapeEvidence::new(vec![
                        CoreFinding::new(
                            "name-cluster",
                            Some("gate/src"),
                            "four modules share an affix",
                        )
                        .expect("core finding"),
                    ])),
                },
                _ => passing_core_observation(id),
            }
        } else {
            passing_core_observation(id)
        };
        CoreArmInput::new(id, source.clone(), core_provenance(id), observation)
            .expect("core fixture arm")
    });
    evaluate_core(&CoreInputs::new(arms.collect()).expect("core fixture battery"))
        .arms()
        .iter()
        .cloned()
        .map(|report| ArmEvidence::Core { report })
        .collect()
}

fn core_provenance(id: GateId) -> CoreProvenance {
    let predecessor = core_declarations()
        .iter()
        .find(|declaration| declaration.id() == id)
        .expect("core declaration")
        .predecessor();
    CoreProvenance::new(
        predecessor.release(),
        predecessor.source(),
        predecessor.observation(),
    )
    .expect("core provenance")
}

fn passing_core_observation(id: GateId) -> CoreObservation {
    let evidence = match id {
        GateId::MergeIntegrity => CoreEvidence::MergeIntegrity(MergeIntegrityEvidence::NoMerge),
        GateId::PlanPhase => CoreEvidence::PlanPhase(PlanPhaseEvidence::NoActiveClaim),
        GateId::RepoShape => CoreEvidence::RepoShape(RepoShapeEvidence::new(Vec::new())),
        GateId::Staging => CoreEvidence::Staging(
            StagingEvidence::new(Vec::<String>::new()).expect("empty staging evidence"),
        ),
        GateId::RepoDocs => CoreEvidence::RepoDocs(
            RepoDocsEvidence::new(vec![
                DocMeasurement::new("AGENTS.md", 10, 20, false).expect("doc measurement"),
            ])
            .expect("repo-doc evidence"),
        ),
        GateId::ConfigKeyValidity => {
            CoreEvidence::ConfigKeyValidity(ConfigKeyEvidence::new(Vec::new()))
        }
        GateId::ConfigFalseDisable => {
            CoreEvidence::ConfigFalseDisable(matching_inventory(&["policy", "pre_commit_builtins"]))
        }
        GateId::ConfigTrackedTrust => {
            CoreEvidence::ConfigTrackedTrust(matching_inventory(&["policy", "pre_commit"]))
        }
        _ => unreachable!("fixture id is in the core interval"),
    };
    CoreObservation::Measured { evidence }
}

fn matching_inventory(path: &[&str]) -> ConfigInventoryEvidence {
    let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
    ConfigInventoryEvidence::new(vec![path.clone()], vec![path], Vec::new())
        .expect("matching inventory")
}

fn toolchain(nonpassing: bool) -> Vec<ArmEvidence> {
    let observations = TOOLCHAIN_ARMS.into_iter().map(|arm| {
        let outcome = if nonpassing {
            match arm.id() {
                GateId::Formatters => ArmOutcome::ToolUnavailable,
                GateId::CanvasWebTypecheck => ArmOutcome::Timeout,
                GateId::PythonTypecheck => ArmOutcome::UnsupportedVersion,
                _ => ArmOutcome::Passed,
            }
        } else {
            ArmOutcome::Passed
        };
        tool_observation(arm, outcome)
    });
    measure_toolchain(observations)
        .expect("toolchain fixture report")
        .arms
        .into_iter()
        .map(|report| ArmEvidence::Toolchain { report })
        .collect()
}

fn tool_observation(
    declaration: crate::toolchain::ToolchainDeclaration,
    outcome: ArmOutcome,
) -> ArmObservation {
    let mut tools: Vec<_> = declaration
        .tools()
        .iter()
        .map(|tool| ToolEvidence::Identified {
            name: tool.name().to_owned(),
            requested_program: tool.program().to_owned(),
            resolved_program: format!("/fixture/bin/{}", tool.program()),
            version: tool.exact_version().to_owned(),
            digest: format!("sha256:fixture-{}", tool.name()),
        })
        .collect();
    if outcome == ArmOutcome::ToolUnavailable {
        let tool = declaration.tools().first().expect("external fixture tool");
        tools[0] = ToolEvidence::Missing {
            name: tool.name().to_owned(),
            requested_program: tool.program().to_owned(),
        };
    }
    if outcome == ArmOutcome::UnsupportedVersion {
        let ToolEvidence::Identified { version, .. } = &mut tools[0] else {
            unreachable!("fixture tool begins identified")
        };
        version.push_str("-unsupported");
    }
    ArmObservation {
        gate_id: declaration.id(),
        outcome,
        reason: format!("fixture {} result", declaration.id()),
        tools,
        findings: Vec::new(),
        exit_code: None,
    }
}

fn metrics(nonpassing: bool) -> Vec<ArmEvidence> {
    STRUCTURAL_ARMS
        .into_iter()
        .map(|arm| {
            let metrics = arm
                .metrics()
                .iter()
                .map(|metric| {
                    let state = if nonpassing && arm.id() == GateId::FileShape {
                        MetricState::Unmeasured {
                            reason: "selected tree was not decoded".to_owned(),
                        }
                    } else {
                        MetricState::measured(metric_measurement(*metric))
                    };
                    let key = if matches!(state, MetricState::Measured { .. }) {
                        metric_key(*metric)
                    } else {
                        MetricKey::Arm
                    };
                    MetricEvidence::new(*metric, key, state).expect("metric fixture evidence")
                })
                .collect();
            ArmEvidence::Metrics {
                report: StructuralArmEvidence::new(arm.id(), metrics)
                    .expect("structural fixture arm"),
            }
        })
        .collect()
}

fn metric_key(metric: MetricId) -> MetricKey {
    match metric {
        MetricId::RoutineCcn | MetricId::RoutineLength => {
            MetricKey::function("src/app.rs", "run").expect("function key")
        }
        _ => MetricKey::path("src/app.rs").expect("path key"),
    }
}

fn metric_measurement(metric: MetricId) -> MetricMeasurement {
    match metric {
        MetricId::FileLines => bounded_metric(metric, 12, "file_loc"),
        MetricId::FileBytes => bounded_metric(metric, 100, "file_bytes"),
        MetricId::RoutineCcn => bounded_metric(metric, 5, "routine_ccn"),
        MetricId::RoutineLength => bounded_metric(metric, 17, "routine_length"),
        MetricId::MagicNumberRegressions => MetricMeasurement::new(
            1,
            ParserEvidence::Native {
                parser: ParserId::CGrammarMagicDeltaV1,
            },
        )
        .with_rule(
            RuleBinding::new(
                "magic.examine_threshold",
                Some("fixture-source-rule"),
                "spice.toml",
                "path src/app.rs",
            )
            .expect("magic policy binding"),
        )
        .with_threshold(10),
        MetricId::MarkdownLinkCaseMismatches => MetricMeasurement::new(
            0,
            ParserEvidence::Native {
                parser: ParserId::MarkdownInlineLinkCaseV1,
            },
        ),
        MetricId::RepositoryDocumentCharacters => {
            unreachable!("repository documents are not structural arms")
        }
    }
}

fn bounded_metric(metric: MetricId, value: u64, field: &str) -> MetricMeasurement {
    let parser = match metric {
        MetricId::FileLines | MetricId::FileBytes => ParserEvidence::Native {
            parser: ParserId::Utf8FileShapeV1,
        },
        MetricId::RoutineCcn | MetricId::RoutineLength => ParserEvidence::External {
            parser: ParserId::LizardCsvV1,
            tool: "lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: "sha256:fixture-lizard".to_owned(),
        },
        _ => unreachable!("bounded structural fixture metric"),
    };
    let kind = match metric {
        MetricId::FileLines => "file_loc",
        MetricId::FileBytes => "file_bytes",
        MetricId::RoutineCcn => "complexity_ccn",
        MetricId::RoutineLength => "complexity_length",
        _ => unreachable!("bounded structural fixture metric"),
    };
    MetricMeasurement::new(value, parser)
        .with_rule(
            RuleBinding::new(
                field,
                Some("fixture-source-rule"),
                "spice.toml",
                "path src/app.rs",
            )
            .expect("metric policy binding"),
        )
        .with_latch(
            LatchBinding::new(
                kind,
                100,
                150,
                0,
                150,
                150,
                false,
                false,
                false,
                false,
                "clear",
                None::<String>,
                None::<String>,
            )
            .expect("metric latch binding"),
        )
}

fn hygiene(nonpassing: bool) -> Vec<ArmEvidence> {
    let observations = HYGIENE_ARMS.into_iter().map(|arm| {
        let outcome = if nonpassing {
            match arm.id() {
                GateId::Reachability => HygieneOutcome::NotApplicable,
                GateId::SymbolReachability => HygieneOutcome::Excluded,
                _ => HygieneOutcome::Passed,
            }
        } else {
            HygieneOutcome::Passed
        };
        hygiene_observation(arm, outcome)
    });
    measure_hygiene(observations)
        .expect("hygiene fixture report")
        .arms
        .into_iter()
        .map(|report| ArmEvidence::Hygiene { report })
        .collect()
}

fn hygiene_observation(
    declaration: crate::hygiene::HygieneDeclaration,
    outcome: HygieneOutcome,
) -> HygieneObservation {
    let evidence = match outcome {
        HygieneOutcome::Passed => completed_hygiene_evidence(declaration),
        HygieneOutcome::NotApplicable => vec![hygiene_language(false)],
        HygieneOutcome::Excluded => vec![AnalysisEvidence::Exclusion(ExclusionEvidence {
            provenance: HygieneProvenance {
                release: "current".to_owned(),
                layer: "repository".to_owned(),
                document: "spice.toml".to_owned(),
                field: "policy.exclude[0]".to_owned(),
            },
            reason: "generated fixture tree".to_owned(),
        })],
        _ => unreachable!("canonical fixture selects pass or admitted absence"),
    };
    HygieneObservation {
        gate_id: declaration.id(),
        outcome,
        reason: format!("{} fixture", declaration.id()),
        evidence,
        findings: Vec::new(),
    }
}

fn completed_hygiene_evidence(
    declaration: crate::hygiene::HygieneDeclaration,
) -> Vec<AnalysisEvidence> {
    let policy_field = match declaration.id() {
        GateId::Reachability => "policy.debt.reachability_test_only",
        GateId::AssertionFreeTests => "policy.debt.assertion_free_tests",
        _ => "compiled:zero-findings",
    };
    let mut evidence = vec![
        hygiene_language(true),
        AnalysisEvidence::Analyzer(AnalyzerEvidence::Identified {
            name: hygiene_analyzer_name(declaration.analyzer()).to_owned(),
            requested: declaration.native_component().to_owned(),
            resolved: format!("compiled:hermetic:{}.rlib", declaration.id()),
            declared_version: "spice-gate-hygiene-v1".to_owned(),
            observed_version: "spice-gate-hygiene-v1".to_owned(),
            digest: format!("sha256:{:02x}", declaration.id() as u8),
        }),
        AnalysisEvidence::PolicyDecision {
            field: policy_field.to_owned(),
            limit: 0,
            observed: 0,
        },
        AnalysisEvidence::Deadline {
            limit_ms: 30_000,
            elapsed_ms: 17,
            state: HygieneStage::Completed,
        },
        AnalysisEvidence::Parse {
            documents: 6,
            state: HygieneStage::Completed,
            failed_path: None,
            diagnostic: None,
        },
        AnalysisEvidence::Measurement {
            state: HygieneStage::Completed,
            stage: "policy-decision".to_owned(),
        },
    ];
    for traversal in declaration.traversals() {
        let item = match traversal {
            HygieneTraversal::ImportGraph => AnalysisEvidence::GraphConstruction {
                production_roots: vec!["spice/cli/entry.py".to_owned()],
                test_roots: vec!["tests/test_cli.py".to_owned()],
                nodes: 9,
                edges: 14,
            },
            HygieneTraversal::SymbolGraph | HygieneTraversal::PythonDeclarations => {
                AnalysisEvidence::SymbolExtraction {
                    declarations: 7,
                    production_references: 4,
                    test_references: 3,
                }
            }
            HygieneTraversal::TestAssertions => AnalysisEvidence::TestClassification {
                files: 2,
                tests: 4,
                assertions: 3,
                helpers: vec!["assert_json".to_owned()],
            },
            HygieneTraversal::PrivateBoundaries => AnalysisEvidence::PrivateBoundaryResolution {
                packages: vec!["spice".to_owned()],
                private_imports: 1,
                private_assertions: 1,
                admitted_exceptions: 0,
                stale_exceptions: 0,
            },
        };
        if !evidence
            .iter()
            .any(|existing| std::mem::discriminant(existing) == std::mem::discriminant(&item))
        {
            evidence.push(item);
        }
    }
    evidence
}

fn hygiene_language(applicable: bool) -> AnalysisEvidence {
    AnalysisEvidence::LanguageApplicability {
        language: "python".to_owned(),
        selected_files: u32::from(applicable) * 6,
        project_markers: if applicable {
            vec!["spice.toml".to_owned()]
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

const fn hygiene_analyzer_name(analyzer: crate::hygiene::HygieneAnalyzer) -> &'static str {
    use crate::hygiene::HygieneAnalyzer;
    match analyzer {
        HygieneAnalyzer::PythonImportGraph => "python-import-graph",
        HygieneAnalyzer::PythonSymbolGraph => "python-symbol-graph",
        HygieneAnalyzer::PythonUnusedSymbols => "python-unused-symbols",
        HygieneAnalyzer::PythonTestAssertions => "python-test-assertions",
        HygieneAnalyzer::PythonPrivateBoundaries => "python-private-boundaries",
    }
}
