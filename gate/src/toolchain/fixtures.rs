fn exact_tools(gate: GateId) -> Vec<ToolEvidence> {
    declaration(gate)
        .expect("fixture gate is declared")
        .tools
        .iter()
        .map(|tool| ToolEvidence::Identified {
            name: tool.name.to_owned(),
            requested_program: tool.program.to_owned(),
            resolved_program: format!("/fixture/bin/{}", tool.program),
            version: tool.exact_version.to_owned(),
            digest: format!("sha256:fixture-{}", tool.name),
        })
        .collect()
}

fn fixture_finding(gate: GateId, suffix: &str) -> ArmFinding {
    let (path, detail) = match gate {
        GateId::Formatters => (
            format!("src/{suffix}.rs"),
            FindingDetail::Formatting {
                formatter: "rustfmt".to_owned(),
            },
        ),
        GateId::LocalPaths => (
            format!("notes/{suffix}.txt"),
            FindingDetail::LocalPath {
                fragment: ["/", "Users", "/alice/work"].concat(),
            },
        ),
        GateId::Taste => (
            format!("notes/{suffix}.md"),
            FindingDetail::Taste {
                word: "simply".to_owned(),
                replacement: "remove the qualifier".to_owned(),
            },
        ),
        GateId::CanvasWebTypecheck => (
            format!("app/src/{suffix}.rs"),
            FindingDetail::CanvasType {
                code: "E0308".to_owned(),
                message: "mismatched types".to_owned(),
            },
        ),
        GateId::JavascriptUnused => (
            format!("web/{suffix}.js"),
            FindingDetail::JavascriptUnused {
                symbol: "orphan".to_owned(),
                declaration_line: 7,
                references: 0,
            },
        ),
        GateId::PythonTypecheck => (
            format!("python/{suffix}.py"),
            FindingDetail::PythonType {
                code: "reportAssignmentType".to_owned(),
                message: "value is not assignable".to_owned(),
            },
        ),
        GateId::EnvPolicy => (
            format!("src/{suffix}.rs"),
            FindingDetail::EnvironmentAccess {
                name: "SECRET_TOKEN".to_owned(),
                access: "unmediated read".to_owned(),
            },
        ),
        GateId::EnvNameLedger => (
            format!("src/{suffix}.rs"),
            FindingDetail::EnvironmentLedger {
                name: "SERVICE_URL".to_owned(),
                declarations: 0,
                references: 2,
            },
        ),
        _ => panic!("fixture gate is outside the toolchain family"),
    };
    ArmFinding {
        path,
        line: Some(7),
        column: Some(3),
        detail,
    }
}

fn fixture_observation(gate: GateId, outcome: ArmOutcome) -> ArmObservation {
    let findings = if outcome == ArmOutcome::Findings {
        vec![fixture_finding(gate, "z"), fixture_finding(gate, "a")]
    } else {
        Vec::new()
    };
    let tools = match outcome {
        ArmOutcome::NotApplicable | ArmOutcome::Excluded => Vec::new(),
        _ => exact_tools(gate),
    };
    let exit_code = (outcome == ArmOutcome::NonzeroAnalysis).then_some(17);
    ArmObservation {
        gate_id: gate,
        outcome,
        reason: format!("fixture {} result", gate.as_str()),
        tools,
        findings,
        exit_code,
    }
}

fn fixture_report_with(replacement: ArmObservation) -> ToolchainReport {
    let observations = TOOLCHAIN_ARMS.into_iter().rev().map(|arm| {
        if arm.id == replacement.gate_id {
            replacement.clone()
        } else {
            fixture_observation(arm.id, ArmOutcome::Passed)
        }
    });
    measure_toolchain(observations).expect("fixture observations are complete")
}

#[test]
fn declarations_are_closed_ordered_explicit_and_never_python_wrappers() {
    let expected = &GateId::ALL[8..16];
    assert_eq!(
        TOOLCHAIN_ARMS.map(|arm| arm.id).as_slice(),
        expected,
        "the toolchain interval must retain predecessor battery order"
    );

    for arm in TOOLCHAIN_ARMS {
        assert!(!arm.native_component.is_empty());
        assert!(!arm.jurisdiction.roots.is_empty());
        assert!(!arm.jurisdiction.includes.is_empty());
        assert!(!arm.jurisdiction.excludes.is_empty());
        assert!(!arm.jurisdiction.applicability.is_empty());
        assert!(!arm.configuration.source.is_empty());
        assert!(!arm.configuration.field.is_empty());
        assert!(!arm.analysis.is_empty());
        for operation in arm.analysis {
            assert!(!operation.contains("python"));
            assert!(!operation.contains(".py"));
            assert!(!operation.contains("spice "));
        }
        for tool in arm.tools {
            assert!(!tool.name.is_empty());
            assert!(!tool.program.is_empty());
            assert!(!tool.version_argv.is_empty());
            assert!(!tool.exact_version.is_empty());
            assert!(!tool.role.is_empty());
            assert_ne!(tool.program, "python");
            assert_ne!(tool.program, "python3");
        }
    }

    assert_eq!(
        declaration(GateId::Formatters)
            .expect("formatters declaration")
            .tools
            .iter()
            .map(|tool| (tool.name, tool.exact_version))
            .collect::<Vec<_>>(),
        [
            ("cargo", "cargo 1.97.1 (c980f4866 2026-06-30)"),
            (
                "rustfmt",
                "rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)"
            ),
        ]
    );
    assert_eq!(
        declaration(GateId::CanvasWebTypecheck)
            .expect("canvas declaration")
            .tools
            .iter()
            .map(|tool| (tool.name, tool.exact_version))
            .collect::<Vec<_>>(),
        [
            ("cargo", "cargo 1.97.1 (c980f4866 2026-06-30)"),
            ("trunk", "trunk 0.21.14"),
        ]
    );
    assert_eq!(
        declaration(GateId::PythonTypecheck)
            .expect("Python typecheck declaration")
            .tools[0]
            .exact_version,
        "pyright 1.1.411"
    );
}

#[test]
fn pass_and_finding_fixtures_cover_every_arm_and_are_deterministic() {
    for outcome in [ArmOutcome::Passed, ArmOutcome::Findings] {
        let observations: Vec<_> = TOOLCHAIN_ARMS
            .into_iter()
            .rev()
            .map(|arm| fixture_observation(arm.id, outcome))
            .collect();
        let first = measure_toolchain(observations.clone()).expect("fixture report");
        let second = measure_toolchain(observations).expect("repeat fixture report");

        assert_eq!(first, second);
        assert_eq!(first.canonical_json(), second.canonical_json());
        assert_eq!(first.version, TOOLCHAIN_REPORT_VERSION);
        assert_eq!(
            first.arms.iter().map(|arm| arm.gate_id).collect::<Vec<_>>(),
            TOOLCHAIN_ARMS.map(|arm| arm.id)
        );
        for measurement in &first.arms {
            assert_eq!(measurement.outcome, outcome);
            assert!(!measurement.reason.is_empty());
            assert!(!measurement.configuration_source.is_empty());
            assert!(!measurement.configuration_field.is_empty());
            assert!(measurement.native_engine.ends_with(NATIVE_ENGINE_VERSION));
            assert!(!measurement.roots.is_empty());
            assert!(!measurement.includes.is_empty());
            assert!(!measurement.excludes.is_empty());
            assert!(!measurement.applicability.is_empty());
            assert_eq!(
                measurement.findings.is_empty(),
                outcome == ArmOutcome::Passed
            );
            assert!(measurement.findings.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}

#[test]
fn generic_nonpass_fixture_states_cover_every_arm_and_only_two_skip() {
    let outcomes = [
        ArmOutcome::NotApplicable,
        ArmOutcome::Excluded,
        ArmOutcome::Timeout,
        ArmOutcome::ParseFailure,
        ArmOutcome::NonzeroAnalysis,
    ];
    for arm in TOOLCHAIN_ARMS {
        for outcome in outcomes {
            let report = fixture_report_with(fixture_observation(arm.id, outcome));
            let measurement = report
                .arms
                .iter()
                .find(|measurement| measurement.gate_id == arm.id)
                .expect("fixture measurement");
            assert_eq!(measurement.outcome, outcome);
            assert_eq!(
                measurement.outcome.is_skip(),
                matches!(outcome, ArmOutcome::NotApplicable | ArmOutcome::Excluded)
            );
            assert_eq!(
                measurement.outcome.is_failure(),
                !matches!(outcome, ArmOutcome::NotApplicable | ArmOutcome::Excluded)
            );
            assert!(!measurement.reason.is_empty());
            assert!(!measurement.configuration_source.is_empty());
            assert!(!measurement.configuration_field.is_empty());
            assert_eq!(
                measurement.exit_code,
                (outcome == ArmOutcome::NonzeroAnalysis).then_some(17)
            );
        }
    }
}

#[test]
fn external_tool_nonpass_fixtures_cover_discovery_and_version_independently() {
    let external = [
        GateId::Formatters,
        GateId::CanvasWebTypecheck,
        GateId::PythonTypecheck,
    ];
    for gate in external {
        let mut unavailable = fixture_observation(gate, ArmOutcome::ToolUnavailable);
        let first = unavailable.tools.first_mut().expect("declared external tool");
        let ToolEvidence::Identified {
            name,
            requested_program,
            ..
        } = first
        else {
            panic!("fixture begins identified");
        };
        *first = ToolEvidence::Missing {
            name: name.clone(),
            requested_program: requested_program.clone(),
        };
        let unavailable_report = fixture_report_with(unavailable);
        let unavailable_measurement = unavailable_report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("unavailable fixture");
        assert_eq!(
            unavailable_measurement.outcome,
            ArmOutcome::ToolUnavailable
        );
        assert!(unavailable_measurement.outcome.is_failure());
        assert!(
            unavailable_measurement
                .tools
                .iter()
                .any(|tool| matches!(tool, ToolEvidence::Missing { .. }))
        );

        let mut unsupported = fixture_observation(gate, ArmOutcome::UnsupportedVersion);
        let ToolEvidence::Identified { version, .. } = unsupported
            .tools
            .first_mut()
            .expect("declared external tool")
        else {
            panic!("fixture begins identified");
        };
        version.push_str("-unsupported");
        let unsupported_report = fixture_report_with(unsupported);
        let unsupported_measurement = unsupported_report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("unsupported fixture");
        assert_eq!(
            unsupported_measurement.outcome,
            ArmOutcome::UnsupportedVersion
        );
        assert!(unsupported_measurement.outcome.is_failure());
    }
}

#[test]
fn typed_state_invariants_refuse_ambiguous_or_incomplete_evidence() {
    let duplicate_gate = TOOLCHAIN_ARMS[0].id;
    assert_eq!(
        measure_toolchain([
            fixture_observation(duplicate_gate, ArmOutcome::Passed),
            fixture_observation(duplicate_gate, ArmOutcome::Passed),
        ]),
        Err(ToolchainError::DuplicateGate(duplicate_gate))
    );
    assert_eq!(
        measure_toolchain(std::iter::empty()),
        Err(ToolchainError::MissingGate(GateId::Formatters))
    );

    let mut bad_set: Vec<_> = TOOLCHAIN_ARMS
        .into_iter()
        .map(|arm| fixture_observation(arm.id, ArmOutcome::Passed))
        .collect();
    let taste = bad_set
        .iter_mut()
        .find(|item| item.gate_id == GateId::Taste)
        .expect("taste fixture");
    taste.outcome = ArmOutcome::Findings;
    assert!(matches!(
        measure_toolchain(bad_set),
        Err(ToolchainError::InvalidFindings {
            gate: GateId::Taste,
            ..
        })
    ));

    let mut bad_version: Vec<_> = TOOLCHAIN_ARMS
        .into_iter()
        .map(|arm| fixture_observation(arm.id, ArmOutcome::Passed))
        .collect();
    let formatters = bad_version
        .iter_mut()
        .find(|item| item.gate_id == GateId::Formatters)
        .expect("formatters fixture");
    let ToolEvidence::Identified { version, .. } = &mut formatters.tools[0] else {
        panic!("formatter tool is identified");
    };
    version.push_str("-drifted");
    assert!(matches!(
        measure_toolchain(bad_version),
        Err(ToolchainError::InvalidToolEvidence {
            gate: GateId::Formatters,
            ..
        })
    ));

    let mut impossible_native_state: Vec<_> = TOOLCHAIN_ARMS
        .into_iter()
        .map(|arm| fixture_observation(arm.id, ArmOutcome::Passed))
        .collect();
    let local_paths = impossible_native_state
        .iter_mut()
        .find(|item| item.gate_id == GateId::LocalPaths)
        .expect("local-paths fixture");
    local_paths.outcome = ArmOutcome::ToolUnavailable;
    assert!(matches!(
        measure_toolchain(impossible_native_state),
        Err(ToolchainError::InvalidToolEvidence {
            gate: GateId::LocalPaths,
            ..
        })
    ));
}

#[test]
fn predecessor_fixture_matrix_compares_scanners_and_refuses_changed_executors() {
    let reports = [
        (
            ArmOutcome::Passed,
            measure_toolchain(
                TOOLCHAIN_ARMS
                    .into_iter()
                    .map(|arm| fixture_observation(arm.id, ArmOutcome::Passed)),
            )
            .expect("passing report"),
        ),
        (
            ArmOutcome::Findings,
            measure_toolchain(
                TOOLCHAIN_ARMS
                    .into_iter()
                    .map(|arm| fixture_observation(arm.id, ArmOutcome::Findings)),
            )
            .expect("finding report"),
        ),
    ];
    for (outcome, report) in reports {
        for current in &report.arms {
            let predecessor_name = match current.gate_id {
                GateId::CanvasWebTypecheck => "serve-web-typecheck",
                gate => gate.as_str(),
            };
            let predecessor = PredecessorObservation {
                gate_name: predecessor_name.to_owned(),
                result: if outcome == ArmOutcome::Passed {
                    PredecessorResult::Passed
                } else {
                    PredecessorResult::Findings(current.findings.clone())
                },
            };
            let comparison = compare_predecessor(&predecessor, current)
                .expect("fixture predecessor comparison is well formed");
            match current.gate_id {
                GateId::Formatters
                | GateId::CanvasWebTypecheck
                | GateId::PythonTypecheck => {
                    let PredecessorComparison::Refused(refusal) = comparison else {
                        panic!("changed executor must carry a refusal");
                    };
                    assert_eq!(refusal.release, PREDECESSOR_GATE_RELEASE);
                    assert_eq!(refusal.gate_id, current.gate_id);
                    assert_eq!(
                        refusal.source_field,
                        format!("policy.pre_commit_builtins.{predecessor_name}")
                    );
                    assert!(!refusal.reason.is_empty());
                    assert!(!refusal.recovery.is_empty());
                }
                _ => assert_eq!(comparison, PredecessorComparison::Equivalent),
            }
        }
    }
}
