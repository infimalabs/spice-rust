use crate::manifest::PREDECESSOR_GATE_RELEASE;

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

/// The same declared tool, resolved to `resolved` and never identified.
fn unprobed_like(evidence: &ToolEvidence, resolved: &str) -> ToolEvidence {
    let ToolEvidence::Identified {
        name,
        requested_program,
        ..
    } = evidence
    else {
        panic!("fixture tools begin identified");
    };
    ToolEvidence::Unprobed {
        name: name.clone(),
        requested_program: requested_program.clone(),
        resolved_program: resolved.to_owned(),
        failure: ProbeFailure::Refused { code: 1 },
    }
}

fn fixture_finding(gate: GateId, suffix: &str) -> ArmFinding {
    if matches!(
        gate,
        GateId::Formatters | GateId::CanvasWebTypecheck | GateId::PythonTypecheck
    ) {
        fixture_external_finding(gate, suffix)
    } else {
        fixture_scanner_finding(gate, suffix)
    }
}

/// One current-shaped finding for a gate an external tool analyzes.
///
/// The column is chosen per gate because these three do not agree on having
/// one, and one shared literal hid that. Each arm below carries the reason its
/// own producer gives.
fn fixture_external_finding(gate: GateId, suffix: &str) -> ArmFinding {
    let (path, column, detail) = match gate {
        // The formatter records that a file is not formatted and carries the
        // earliest differing region as a line, so its analysis reports no
        // column at all and neither does this.
        GateId::Formatters => (
            format!("src/{suffix}.rs"),
            None,
            FindingDetail::Formatting {
                formatter: "rustfmt".to_owned(),
            },
        ),
        // The canvas analysis parses `path:line:column` out of the location its
        // tool printed and keeps only a positive column, so a positive literal
        // is the shape it always produces.
        GateId::CanvasWebTypecheck => (
            format!("app/src/{suffix}.rs"),
            Some(3),
            FindingDetail::CanvasWebOutput {
                code: "E0308".to_owned(),
                message: "mismatched types".to_owned(),
            },
        ),
        // Python typechecking has no compiled analysis yet, which is why
        // `owed_analyzer` still names the one it is owed, so there is no
        // producer to ask. Pyright's own diagnostics carry a column, and this
        // literal stands in for it until the analysis that parses one lands.
        GateId::PythonTypecheck => (
            format!("python/{suffix}.py"),
            Some(3),
            FindingDetail::PythonType {
                code: "reportAssignmentType".to_owned(),
                message: "value is not assignable".to_owned(),
            },
        ),
        _ => panic!("fixture external gate is outside the toolchain family"),
    };
    ArmFinding {
        path: Some(path),
        line: Some(7),
        column,
        detail,
    }
}

/// The line the environment fixture finding was read from.
const FIXTURE_ENV_SOURCE: &str = "let hooks = std::env::var(\"SPICE_HOOKS\");"; // env-policy: allow
/// The line the local-path fixture finding was read from.
const FIXTURE_MARKED_LINE: &str = concat!("// see /", "Users", "/somebody/devel");
/// The line the taste fixture finding was read from.
const FIXTURE_PROSE_LINE: &str = "one orphaned paragraph";

/// One current-shaped scanner finding, carrying the column its scanner derives.
///
/// Three scanners report one finding per occurrence and take its column from
/// the line they matched in; the rest name a file or the repository and report
/// none. The fixture derives those three the way each scanner does instead of
/// naming a number, so the value here is one a run could produce and the
/// equality it feeds is not two empty fields agreeing.
///
/// It does not stand watch over the producers. Parity erases the column on
/// both sides, so a scanner that stopped reporting one, or a predecessor
/// conversion that started inventing one, would pass unremarked here. What
/// this fixture holds is that the current side arrives with a column at all.
fn fixture_scanner_finding(gate: GateId, suffix: &str) -> ArmFinding {
    let (path, column, detail) = match gate {
        GateId::LocalPaths => (
            Some(format!("notes/{suffix}.txt")),
            fixture_column(FIXTURE_MARKED_LINE, LOCAL_PATH_MARKER),
            FindingDetail::LocalPath,
        ),
        GateId::Taste => (
            Some(format!("notes/{suffix}.md")),
            fixture_column(FIXTURE_PROSE_LINE, "orphaned"),
            FindingDetail::Taste {
                word: "orphaned".to_owned(),
                replacement: "loose".to_owned(),
            },
        ),
        GateId::JavascriptUnused => (
            Some(format!("web/{suffix}.js")),
            None,
            FindingDetail::JavascriptUnused {
                symbol: "orphan".to_owned(),
                declaration_kind: "function".to_owned(),
                status: JavascriptUnusedStatus::TestOnly,
                reason: "references_only_in_tests".to_owned(),
                reference_count: 1,
                test_reference_count: 1,
            },
        ),
        GateId::CsharpUnusedCandidates => (
            Some(format!("dotnet/{suffix}.cs")),
            None,
            FindingDetail::CsharpUnused {
                symbol: "Loose".to_owned(),
                declaration_kind: "method".to_owned(),
                status: CsharpUnusedStatus::CandidateUnused,
                reason: "no_references_outside_declaration".to_owned(),
                reference_count: 1,
            },
        ),
        GateId::EnvPolicy => (
            Some(format!("src/{suffix}.rs")),
            fixture_access_column(),
            FindingDetail::EnvironmentAccess {
                name: "SPICE_HOOKS".to_owned(), // env-policy: allow
                source: FIXTURE_ENV_SOURCE.to_owned(),
            },
        ),
        GateId::EnvNameLedger => (
            None,
            None,
            FindingDetail::EnvironmentLedger {
                mismatch: EnvironmentLedgerKind::Unaccounted,
                name: "SPICE_HOOKS".to_owned(), // env-policy: allow
                references: vec![
                    SourceLocus {
                        path: format!("src/{suffix}.rs"),
                        line: 7,
                    },
                    SourceLocus {
                        path: format!("web/{suffix}.js"),
                        line: 11,
                    },
                ],
            },
        ),
        _ => panic!("fixture scanner gate is outside the toolchain family"),
    };
    ArmFinding {
        line: path.as_ref().map(|_| 7),
        path,
        column,
        detail,
    }
}

/// The column one scanner reports for `needle` in a line it matched.
///
/// The needle must be present and `column_of` must place it, because a `None`
/// reached by either failing is exactly the empty column this fixture exists to
/// stop producing.
fn fixture_column(line: &str, needle: &str) -> Option<u32> {
    let start = line
        .find(needle)
        .unwrap_or_else(|| panic!("fixture line {line:?} must contain {needle:?}"));
    Some(column_of(line, start).expect("a byte offset inside the line has a column"))
}

/// The column the compiled environment scanner reports for the fixture access.
fn fixture_access_column() -> Option<u32> {
    let accesses = environment::accesses(environment::family("rs"), FIXTURE_ENV_SOURCE);
    let access = accesses
        .iter()
        .find(|access| access.name == "SPICE_HOOKS") // env-policy: allow
        .expect("the fixture source line commits the environment access it reports");
    Some(access.column)
}

fn predecessor_finding(gate: GateId, suffix: &str) -> PredecessorFinding {
    match gate {
        GateId::Formatters => PredecessorFinding::FormattingOutput {
            bytes: format!("ruff finding in python/{suffix}.py").into_bytes(),
        },
        GateId::LocalPaths => PredecessorFinding::LocalPath {
            path: format!("notes/{suffix}.txt"),
            line: 7,
        },
        GateId::Taste => PredecessorFinding::Taste {
            path: format!("notes/{suffix}.md"),
            line: 7,
            word: "orphaned".to_owned(),
            suggestion: "loose".to_owned(),
        },
        GateId::CanvasWebTypecheck => PredecessorFinding::CanvasWebOutput {
            bytes: format!("TS2322 in spice/serve/static/{suffix}.js").into_bytes(),
        },
        GateId::JavascriptUnused => PredecessorFinding::JavascriptUnused {
            path: format!("web/{suffix}.js"),
            line: 7,
            declaration_kind: "function".to_owned(),
            symbol: "orphan".to_owned(),
            status: JavascriptUnusedStatus::TestOnly,
            reason: "references_only_in_tests".to_owned(),
            reference_count: 1,
            test_reference_count: 1,
        },
        GateId::PythonTypecheck => PredecessorFinding::PythonTypeOutput {
            bytes: format!("reportAssignmentType in python/{suffix}.py").into_bytes(),
        },
        GateId::EnvPolicy => PredecessorFinding::EnvironmentAccess {
            path: format!("src/{suffix}.rs"),
            line: 7,
            name: "SPICE_HOOKS".to_owned(), // env-policy: allow
            source: FIXTURE_ENV_SOURCE.to_owned(),
        },
        GateId::EnvNameLedger => PredecessorFinding::EnvironmentLedger {
            kind: EnvironmentLedgerKind::Unaccounted,
            name: "SPICE_HOOKS".to_owned(), // env-policy: allow
            references: vec![
                SourceLocus {
                    path: format!("web/{suffix}.js"),
                    line: 11,
                },
                SourceLocus {
                    path: format!("src/{suffix}.rs"),
                    line: 7,
                },
            ],
        },
        _ => panic!("fixture gate is outside the toolchain family"),
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
    fixture_report_result_with(replacement).expect("fixture observations are complete")
}

fn fixture_report_result_with(
    replacement: ArmObservation,
) -> Result<ToolchainReport, ToolchainError> {
    let observations = TOOLCHAIN_ARMS.into_iter().rev().map(|arm| {
        if arm.id == replacement.gate_id {
            replacement.clone()
        } else {
            fixture_observation(arm.id, ArmOutcome::Passed)
        }
    });
    measure_toolchain(observations)
}

#[test]
fn declarations_are_closed_ordered_explicit_and_never_python_wrappers() {
    let expected = &GateId::ALL[8..17];
    assert_eq!(
        TOOLCHAIN_ARMS.map(|arm| arm.id).as_slice(),
        expected,
        "the toolchain interval must retain predecessor battery order"
    );

    for arm in TOOLCHAIN_ARMS {
        assert_eq!(arm.measurement_engine, MeasurementEngine::ToolchainV1);
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
}

#[test]
fn external_tool_declarations_are_exact() {
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
fn packaged_scanner_profiles_match_release_0_31_0() {
    assert_eq!(LOCAL_PATH_MARKER, ["/", "Users", "/"].concat());
    assert_eq!(
        PACKAGED_TASTE_WORDS,
        &[
            ("hallucinat*", "confabulate"),
            ("master", "main, primary, or leader"),
            ("orphan*", "loose"),
            ("whitelist", "allowlist"),
            ("whitelists", "allowlists"),
            ("whitelisted", "allowlisted"),
            ("whitelisting", "allowlisting"),
            ("blacklist", "blocklist"),
            ("blacklists", "blocklists"),
            ("blacklisted", "blocklisted"),
            ("blacklisting", "blocklisting"),
        ]
    );
    assert_eq!(
        PACKAGED_JAVASCRIPT_EXEMPTIONS,
        &[(
            "spice/serve/static/app.mosaic-event-log.js",
            "mosaicReplayEventLog",
            concat!(
                "shared browser replay oracle that exercises the production event-log ",
                "branch implementation without duplicating that algorithm into tests"
            ),
        )]
    );
    assert_eq!(PACKAGED_TEST_PATHS, &["tests"]);
    assert_eq!(
        PACKAGED_SCANNER_EXCLUDES,
        &[
            "any path component equal to .git",
            "any path component starting with .spice",
            "any path component equal to .venv",
            "any path component equal to .ruff_cache",
            "any path component equal to .pytest_cache",
            "any path component equal to __pycache__",
            "any path component equal to node_modules",
            "any path component equal to site-packages",
        ]
    );
}

#[test]
fn packaged_environment_jurisdiction_matches_release_0_31_0() {
    assert_eq!(
        PACKAGED_ENV_INCLUDES,
        &[
            "**/*.c",
            "**/*.cc",
            "**/*.cpp",
            "**/*.cs",
            "**/*.go",
            "**/*.h",
            "**/*.hpp",
            "**/*.java",
            "**/*.js",
            "**/*.kt",
            "**/*.m",
            "**/*.mm",
            "**/*.rs",
            "**/*.scala",
            "**/*.swift",
            "**/*.ts",
            "**/*.lua",
            "**/*.php",
            "**/*.py",
            "**/*.rb",
            "**/*.bash",
            "**/*.sh",
            "**/*.zsh",
        ]
    );
    assert_eq!(PACKAGED_ENV_PROFILE.includes, PACKAGED_ENV_INCLUDES);
    assert_eq!(PACKAGED_ENV_PROFILE.excludes, PACKAGED_SCANNER_EXCLUDES);
    assert_eq!(PACKAGED_ENV_PROFILE.allow_marker, "env-policy: allow");
    assert_eq!(
        PACKAGED_ENV_PROFILE.name_patterns,
        &[
            "SPICE_[A-Z0-9_]+",
            "CODEX_THREAD_ID",        // env-policy: allow
            "CLAUDE_CODE_SESSION_ID", // env-policy: allow
        ]
    );
    assert_eq!(PACKAGED_ENV_PROFILE.self_path_suffix, "studies/envpolicy.py");
    const { assert!(PACKAGED_ENV_PROFILE.access_gate) };
    assert!(PACKAGED_ENV_PROFILE.declared_names.is_empty());
    assert_eq!(PACKAGED_ENV_PROFILE.baseline, None);
}

#[test]
fn packaged_environment_access_profile_matches_release_0_31_0() {
    assert_eq!(
        PACKAGED_ENV_PROFILE.family_suffixes,
        &[
            ("python", [".py"].as_slice()),
            ("csharp", [".cs"].as_slice()),
            ("lua", [".lua"].as_slice()),
            ("shell", [".bash", ".sh", ".zsh"].as_slice()),
            ("javascript", [".js", ".ts"].as_slice()),
        ]
    );
    assert_eq!(
        PACKAGED_ENV_PROFILE.finding_names,
        &[
            ("python", "os env access"),
            ("csharp", "environment env access"),
            ("lua", "lua env access"),
            ("shell", "shell env access"),
            ("javascript", "process.env access"),
        ]
    );
    assert_eq!(
        PACKAGED_ENV_PROFILE.access_patterns,
        &[
            (
                "python",
                [r"\bos\.(?:environ|getenv|putenv|unsetenv)\b"].as_slice(),
            ),
            (
                "csharp",
                [concat!(
                    r"\b(?:System\.)?Environment\.",
                    r"(?:GetEnvironmentVariable|SetEnvironmentVariable)\b"
                )]
                .as_slice(),
            ),
            ("lua", [r"\bos\.getenv\b"].as_slice()),
            (
                "shell",
                [
                    concat!(
                        r"(?<!\\)\$(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+|\{",
                        r"(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+)\})"
                    ),
                    r"\bexport\s+(?:[A-Za-z][A-Za-z0-9_]*|_[A-Za-z0-9_]+)=",
                ]
                .as_slice(),
            ),
            ("javascript", [r"\bprocess\.env\b"].as_slice()),
        ]
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
            assert_eq!(
                measurement.measurement_engine,
                MeasurementEngine::ToolchainV1
            );
            assert_eq!(
                measurement.tree_input,
                declaration(measurement.gate_id)
                    .expect("measurement declaration")
                    .jurisdiction
                    .input
            );
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
fn external_tool_unavailability_refuses_mixed_version_drift() {
    for gate in [GateId::Formatters, GateId::CanvasWebTypecheck] {
        let mut mixed = fixture_observation(gate, ArmOutcome::ToolUnavailable);
        let ToolEvidence::Identified {
            name,
            requested_program,
            ..
        } = &mixed.tools[0]
        else {
            panic!("fixture begins identified");
        };
        mixed.tools[0] = ToolEvidence::Missing {
            name: name.clone(),
            requested_program: requested_program.clone(),
        };
        let ToolEvidence::Identified { version, .. } = &mut mixed.tools[1] else {
            panic!("second fixture tool is identified");
        };
        version.push_str("-unsupported");
        assert!(matches!(
            fixture_report_result_with(mixed),
            Err(ToolchainError::InvalidToolEvidence { gate: invalid, .. }) if invalid == gate
        ));
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

    let mut impossible_tool_free_state: Vec<_> = TOOLCHAIN_ARMS
        .into_iter()
        .map(|arm| fixture_observation(arm.id, ArmOutcome::Passed))
        .collect();
    let local_paths = impossible_tool_free_state
        .iter_mut()
        .find(|item| item.gate_id == GateId::LocalPaths)
        .expect("local-paths fixture");
    local_paths.outcome = ArmOutcome::ToolUnavailable;
    assert!(matches!(
        measure_toolchain(impossible_tool_free_state),
        Err(ToolchainError::InvalidToolEvidence {
            gate: GateId::LocalPaths,
            ..
        })
    ));
}

/// A program that never answered refuses its arm, and can do nothing else.
///
/// `ToolUnavailable` beside exact versions is the only pairing it has. Riding a
/// pass would launder a tool that produced no version into a measurement taken
/// with it, and a row naming no file claims a program was resolved while
/// carrying nothing an operator could go look at.
#[test]
fn unprobed_evidence_cannot_ride_a_pass_or_name_no_file() {
    let gate = GateId::Formatters;

    let mut passed = fixture_observation(gate, ArmOutcome::Passed);
    passed.tools[0] = unprobed_like(&passed.tools[0], "/fixture/bin/unprobed");
    assert!(matches!(
        fixture_report_result_with(passed),
        Err(ToolchainError::InvalidToolEvidence { gate: invalid, .. }) if invalid == gate
    ));

    let mut unnamed = fixture_observation(gate, ArmOutcome::ToolUnavailable);
    unnamed.tools[0] = unprobed_like(&unnamed.tools[0], "");
    assert!(matches!(
        fixture_report_result_with(unnamed),
        Err(ToolchainError::InvalidToolEvidence { gate: invalid, .. }) if invalid == gate
    ));
}

#[test]
fn structured_finding_identity_refuses_missing_loci_and_inconsistent_detail() {
    let mut missing_path = fixture_observation(GateId::LocalPaths, ArmOutcome::Findings);
    missing_path.findings[0].path = None;
    assert!(matches!(
        fixture_report_result_with(missing_path),
        Err(ToolchainError::InvalidFindings {
            gate: GateId::LocalPaths,
            ..
        })
    ));

    let mut empty_ledger = fixture_observation(GateId::EnvNameLedger, ArmOutcome::Findings);
    let FindingDetail::EnvironmentLedger { references, .. } =
        &mut empty_ledger.findings[0].detail
    else {
        panic!("ledger fixture has ledger detail");
    };
    references.clear();
    assert!(matches!(
        fixture_report_result_with(empty_ledger),
        Err(ToolchainError::InvalidFindings {
            gate: GateId::EnvNameLedger,
            ..
        })
    ));

    let mut bad_javascript = fixture_observation(GateId::JavascriptUnused, ArmOutcome::Findings);
    let FindingDetail::JavascriptUnused { reason, .. } =
        &mut bad_javascript.findings[0].detail
    else {
        panic!("JavaScript fixture has JavaScript detail");
    };
    *reason = "different reason".to_owned();
    assert!(matches!(
        fixture_report_result_with(bad_javascript),
        Err(ToolchainError::InvalidFindings {
            gate: GateId::JavascriptUnused,
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
            // Release 0.31.0 ran no step for an arm this workspace grew past
            // it, so an observation claiming one is refused outright instead of
            // being compared against anything.
            if !current.gate_id.predecessor_declared() {
                let fabricated = PredecessorObservation {
                    gate_name: current.gate_id.as_str().to_owned(),
                    configuration: PredecessorConfiguration::PackagedDefaults,
                    result: PredecessorResult::Passed,
                };
                assert_eq!(
                    compare_predecessor(&fabricated, current),
                    Err(PredecessorComparisonError::UnknownGate(
                        current.gate_id.as_str().to_owned()
                    ))
                );
                continue;
            }
            let predecessor_name = match current.gate_id {
                GateId::CanvasWebTypecheck => "serve-web-typecheck",
                gate => gate.as_str(),
            };
            let predecessor = PredecessorObservation {
                gate_name: predecessor_name.to_owned(),
                configuration: PredecessorConfiguration::PackagedDefaults,
                result: if outcome == ArmOutcome::Passed {
                    PredecessorResult::Passed
                } else {
                    PredecessorResult::Findings(vec![
                        predecessor_finding(current.gate_id, "z"),
                        predecessor_finding(current.gate_id, "a"),
                    ])
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

#[test]
fn predecessor_field_overrides_refuse_with_exact_source_and_recovery() {
    let cases = [
        (GateId::LocalPaths, "local-paths", "policy.exclude[0]"),
        (GateId::Taste, "taste", "policy.taste.words.orphan*"),
        (
            GateId::CanvasWebTypecheck,
            "serve-web-typecheck",
            "policy.exclude[0]",
        ),
        (
            GateId::JavascriptUnused,
            "javascript-unused",
            "policy.test_paths[0]",
        ),
        (GateId::EnvPolicy, "env-policy", "policy.languages.env[0]"),
        (GateId::EnvPolicy, "env-policy", "policy.exclude[0]"),
        (
            GateId::EnvNameLedger,
            "env-name-ledger",
            "policy.env_names[0]",
        ),
        (
            GateId::EnvNameLedger,
            "env-name-ledger",
            "policy.exclude[0]",
        ),
    ];
    for (gate, predecessor_name, source_field) in cases {
        let report = fixture_report_with(fixture_observation(gate, ArmOutcome::Passed));
        let current = report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("scanner measurement");
        let predecessor = PredecessorObservation {
            gate_name: predecessor_name.to_owned(),
            configuration: PredecessorConfiguration::Override {
                source_field: source_field.to_owned(),
            },
            result: PredecessorResult::Passed,
        };
        let comparison = compare_predecessor(&predecessor, current)
            .expect("configured predecessor returns a typed refusal");
        let PredecessorComparison::Refused(refusal) = comparison else {
            panic!("configured predecessor scanner must refuse");
        };
        assert_eq!(refusal.release, PREDECESSOR_GATE_RELEASE);
        assert_eq!(refusal.gate_id, gate);
        assert_eq!(refusal.source_field, source_field);
        assert!(!refusal.reason.is_empty());
        assert!(refusal.recovery.contains(gate.as_str()));
    }
}

#[test]
fn predecessor_override_field_is_validated_before_every_refusal() {
    for (gate, predecessor_name) in [
        (GateId::EnvPolicy, "env-policy"),
        (GateId::CanvasWebTypecheck, "serve-web-typecheck"),
    ] {
        let report = fixture_report_with(fixture_observation(gate, ArmOutcome::Passed));
        let current = report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("fixture measurement");
        for source_field in ["", "policy.exclude\n[0]"] {
            let predecessor = PredecessorObservation {
                gate_name: predecessor_name.to_owned(),
                configuration: PredecessorConfiguration::Override {
                    source_field: source_field.to_owned(),
                },
                result: PredecessorResult::Passed,
            };
            assert_eq!(
                compare_predecessor(&predecessor, current),
                Err(PredecessorComparisonError::InvalidConfigurationField(gate))
            );
        }
    }
}

#[test]
fn predecessor_finding_shape_is_validated_before_executor_refusal() {
    let gate = GateId::Formatters;
    let report = fixture_report_with(fixture_observation(gate, ArmOutcome::Findings));
    let current = report
        .arms
        .iter()
        .find(|measurement| measurement.gate_id == gate)
        .expect("formatter measurement");
    let results = [
        PredecessorResult::Findings(Vec::new()),
        PredecessorResult::Findings(vec![PredecessorFinding::FormattingOutput {
            bytes: Vec::new(),
        }]),
        PredecessorResult::Findings(vec![predecessor_finding(GateId::LocalPaths, "a")]),
    ];
    for result in results {
        let predecessor = PredecessorObservation {
            gate_name: "formatters".to_owned(),
            configuration: PredecessorConfiguration::PackagedDefaults,
            result,
        };
        assert_eq!(
            compare_predecessor(&predecessor, current),
            Err(PredecessorComparisonError::ResultMismatch(gate))
        );
    }
}

#[test]
fn predecessor_differences_in_javascript_and_ledger_identity_do_not_compare_equal() {
    for gate in [GateId::JavascriptUnused, GateId::EnvNameLedger] {
        let report = fixture_report_with(fixture_observation(gate, ArmOutcome::Findings));
        let current = report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("finding measurement");
        let mut findings = vec![
            predecessor_finding(gate, "z"),
            predecessor_finding(gate, "a"),
        ];
        match &mut findings[0] {
            PredecessorFinding::JavascriptUnused { status, .. } => {
                *status = JavascriptUnusedStatus::CandidateUnused;
            }
            PredecessorFinding::EnvironmentLedger { references, .. } => {
                references[0].line += 1;
            }
            _ => unreachable!("fixture selected a complete-identity scanner"),
        }
        let predecessor = PredecessorObservation {
            gate_name: gate.as_str().to_owned(),
            configuration: PredecessorConfiguration::PackagedDefaults,
            result: PredecessorResult::Findings(findings),
        };
        assert_eq!(
            compare_predecessor(&predecessor, current),
            Err(PredecessorComparisonError::ResultMismatch(gate))
        );
    }
}

/// The three scanners that report a column, and the one field parity drops.
///
/// Every current finding here carries the column its scanner derives, so
/// equality proves the comparison excludes the field rather than that both
/// producers happened to leave it empty. Moving one line then proves the loci
/// the comparison does hold are still compared.
#[test]
fn column_reporting_scanners_compare_equal_across_the_column_alone() {
    for gate in [GateId::LocalPaths, GateId::Taste, GateId::EnvPolicy] {
        let report = fixture_report_with(fixture_observation(gate, ArmOutcome::Findings));
        let current = report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == gate)
            .expect("scanner measurement");
        assert!(
            current
                .findings
                .iter()
                .all(|finding| finding.column.is_some()),
            "{} reports a column, so equality here is not two empty fields agreeing",
            gate.as_str()
        );
        let findings = vec![
            predecessor_finding(gate, "z"),
            predecessor_finding(gate, "a"),
        ];
        let predecessor = PredecessorObservation {
            gate_name: gate.as_str().to_owned(),
            configuration: PredecessorConfiguration::PackagedDefaults,
            result: PredecessorResult::Findings(findings.clone()),
        };
        assert_eq!(
            compare_predecessor(&predecessor, current),
            Ok(PredecessorComparison::Equivalent)
        );

        let mut moved = findings;
        match &mut moved[0] {
            PredecessorFinding::LocalPath { line, .. }
            | PredecessorFinding::Taste { line, .. }
            | PredecessorFinding::EnvironmentAccess { line, .. } => *line += 1,
            _ => unreachable!("fixture selected a column-reporting scanner"),
        }
        let predecessor = PredecessorObservation {
            gate_name: gate.as_str().to_owned(),
            configuration: PredecessorConfiguration::PackagedDefaults,
            result: PredecessorResult::Findings(moved),
        };
        assert_eq!(
            compare_predecessor(&predecessor, current),
            Err(PredecessorComparisonError::ResultMismatch(gate))
        );
    }
}

#[test]
fn packaged_environment_evidence_covers_csharp_typescript_and_shell() {
    let sources = [
        (
            "src/service.cs",
            "Environment.GetEnvironmentVariable(\"SPICE_HOOKS\")", // env-policy: allow
        ),
        ("web/client.ts", "process.env[\"SPICE_HOOKS\"]"), // env-policy: allow
        (
            "scripts/run.sh",
            "name='SPICE_HOOKS'; echo \"$SPICE_HOOKS\"", // env-policy: allow
        ),
    ];
    let findings: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(index, (path, source))| ArmFinding {
            path: Some((*path).to_owned()),
            line: Some((index + 1) as u32),
            column: None,
            detail: FindingDetail::EnvironmentAccess {
                name: "SPICE_HOOKS".to_owned(), // env-policy: allow
                source: (*source).to_owned(),
            },
        })
        .collect();
    let mut observation = fixture_observation(GateId::EnvPolicy, ArmOutcome::Findings);
    observation.findings = findings;
    let report = fixture_report_with(observation);
    let current = report
        .arms
        .iter()
        .find(|measurement| measurement.gate_id == GateId::EnvPolicy)
        .expect("environment measurement");
    let predecessor = PredecessorObservation {
        gate_name: "env-policy".to_owned(),
        configuration: PredecessorConfiguration::PackagedDefaults,
        result: PredecessorResult::Findings(
            sources
                .iter()
                .enumerate()
                .map(|(index, (path, source))| PredecessorFinding::EnvironmentAccess {
                    path: (*path).to_owned(),
                    line: (index + 1) as u32,
                    name: "SPICE_HOOKS".to_owned(), // env-policy: allow
                    source: (*source).to_owned(),
                })
                .collect(),
        ),
    };
    assert_eq!(
        compare_predecessor(&predecessor, current),
        Ok(PredecessorComparison::Equivalent)
    );
}
