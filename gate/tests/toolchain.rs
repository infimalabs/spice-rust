use std::fs;
use std::path::{Path, PathBuf};

use gate::{
    ArmEvidence, ArmFinding, ArmOutcome, BatteryState, CsharpUnusedStatus, FindingDetail, GateId,
    JavascriptUnusedStatus, LOCAL_PATH_MARKER, PACKAGED_JAVASCRIPT_EXEMPTIONS,
    PredecessorComparison, PredecessorComparisonError, PredecessorConfiguration,
    PredecessorFinding, PredecessorObservation, PredecessorResult, ProbeFailure, TOOLCHAIN_ARMS,
    TOOLCHAIN_REPORT_VERSION, ToolEvidence, compare_predecessor, measure_toolchain,
};
use wire::{Digest, Domain};

mod support;

use support::toolchain::*;
use support::{Tree, fixture_column, git_commit, inapplicable_without_tools};

#[test]
fn acquisition_over_a_real_tree_yields_a_report_measure_toolchain_admits() {
    let repository = scanning_repository(
        "toolchain-real",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let search_path = toolchain_directory();
    let observations = acquired(
        &repository,
        Some(search_path.clone().into_os_string()),
        &[SOURCE],
    );

    let mut identified = 0_usize;
    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        for (requirement, evidence) in declaration.tools().iter().zip(&observation.tools) {
            let ToolEvidence::Identified {
                resolved_program,
                version,
                digest,
                ..
            } = evidence
            else {
                continue;
            };
            identified += 1;
            let resolved = PathBuf::from(resolved_program);
            assert_eq!(
                resolved.parent(),
                Some(search_path.as_path()),
                "{} resolved {} outside the captured search path",
                declaration.id().as_str(),
                requirement.program()
            );
            let bytes = fs::read(&resolved).expect("read the resolved executable");
            assert_eq!(
                digest,
                Digest::compute(Domain::GateTool, &bytes).as_str(),
                "{} recorded a digest the resolved bytes do not produce",
                requirement.program()
            );
            assert_eq!(
                version,
                &probe_directly(&resolved, requirement.version_argv()),
                "{} recorded something other than what the probe printed",
                requirement.program()
            );
        }
    }
    assert!(
        identified > 0,
        "the captured search path resolved no program, so nothing was proven"
    );

    let report = measure_toolchain(observations).expect("acquired observations are admitted");
    assert_eq!(report.version, TOOLCHAIN_REPORT_VERSION);
    assert_eq!(
        report
            .arms
            .iter()
            .map(|arm| arm.gate_id)
            .collect::<Vec<_>>(),
        TOOLCHAIN_ARMS
            .into_iter()
            .map(|declaration| declaration.id())
            .collect::<Vec<_>>(),
        "the report is complete and in compiled order"
    );
}

#[test]
fn a_program_absent_from_the_captured_search_path_is_unavailable_with_missing_evidence() {
    let repository = scanning_repository(
        "toolchain-absent",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let empty = Tree::new("toolchain-empty-path");
    let observations = acquired(
        &repository,
        Some(empty.root.clone().into_os_string()),
        &[SOURCE, PROSE],
    );

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        if declaration.tools().is_empty() {
            let expected = if COMPILED_ARMS.contains(&declaration.id()) {
                ArmOutcome::Passed
            } else {
                ArmOutcome::Unsupported
            };
            assert_eq!(
                observation.outcome,
                expected,
                "{} declares no tool, so an empty search path cannot change it",
                declaration.id().as_str()
            );
            continue;
        }
        if inapplicable_without_tools(declaration, observation) {
            continue;
        }
        assert_eq!(
            observation.outcome,
            ArmOutcome::ToolUnavailable,
            "{} resolved a program the empty search path cannot hold",
            declaration.id().as_str()
        );
        for (requirement, evidence) in declaration.tools().iter().zip(&observation.tools) {
            let ToolEvidence::Missing {
                requested_program, ..
            } = evidence
            else {
                panic!(
                    "{} identified {} from an empty search path",
                    declaration.id().as_str(),
                    requirement.program()
                );
            };
            assert_eq!(requested_program, requirement.program());
            assert!(
                observation.reason.contains(requirement.program()),
                "{} named no unresolved program in {:?}",
                declaration.id().as_str(),
                observation.reason
            );
        }
    }

    measure_toolchain(observations).expect("unavailable observations are admitted");
}

/// An installed program that refuses its probe is unusable, never absent.
///
/// Every declared program is present and every one of them refuses, which is
/// the state the old acquisition could not tell apart from an empty search
/// path. The arm still refuses the battery, and admission still takes it, so
/// the distinction costs nothing: what changes is that the report names the
/// exact file to look at and what it did, instead of sending an operator to
/// install a program that is already there.
#[test]
fn a_declared_program_that_refuses_its_probe_is_unavailable_and_not_missing() {
    let repository = scanning_repository(
        "toolchain-refused",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let stubs = refusing_stub_directory("toolchain-refused-probes");
    let observations = acquired(
        &repository,
        Some(stubs.root.clone().into_os_string()),
        &[SOURCE, PROSE],
    );

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        if declaration.tools().is_empty() {
            continue;
        }
        if inapplicable_without_tools(declaration, observation) {
            continue;
        }
        assert_eq!(
            observation.outcome,
            ArmOutcome::ToolUnavailable,
            "{} read a refusing program as {:?}",
            declaration.id().as_str(),
            observation.reason
        );
        for (requirement, evidence) in declaration.tools().iter().zip(&observation.tools) {
            let ToolEvidence::Unprobed {
                resolved_program,
                failure,
                ..
            } = evidence
            else {
                panic!(
                    "{} called {} absent though the search path holds it",
                    declaration.id().as_str(),
                    requirement.program()
                );
            };
            assert_eq!(
                Path::new(resolved_program),
                stubs.path(requirement.program()),
                "{} named a file the search path does not hold",
                declaration.id().as_str()
            );
            assert_eq!(
                *failure,
                ProbeFailure::Refused {
                    code: REFUSING_STATUS
                },
                "{} lost the status {} refused with",
                declaration.id().as_str(),
                requirement.program()
            );
            assert!(
                observation.reason.contains(resolved_program),
                "{} refused without naming the file to look at: {:?}",
                declaration.id().as_str(),
                observation.reason
            );
        }
    }

    measure_toolchain(observations).expect("unprobed observations are admitted");
    repository.remove();
    stubs.remove();
}

#[test]
fn a_program_that_leaves_its_exact_version_contract_is_unsupported_version() {
    let repository = scanning_repository(
        "toolchain-drift",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let stubs = stub_directory("toolchain-drifted-cargo");
    let observations = acquired(
        &repository,
        Some(stubs.root.clone().into_os_string()),
        &[SOURCE, PROSE],
    );

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        if inapplicable_without_tools(declaration, observation) {
            continue;
        }
        let drifted = declaration
            .tools()
            .iter()
            .any(|requirement| requirement.program() == DRIFTED_PROGRAM);
        let expected = if drifted {
            ArmOutcome::UnsupportedVersion
        } else if COMPILED_ARMS.contains(&declaration.id()) {
            ArmOutcome::Passed
        } else {
            ArmOutcome::Unsupported
        };
        assert_eq!(
            observation.outcome,
            expected,
            "{} read {} as {:?}",
            declaration.id().as_str(),
            DRIFTED_PROGRAM,
            observation.reason
        );
        if drifted {
            assert!(
                observation.reason.contains(DRIFTED_VERSION),
                "{} hid the version it actually observed in {:?}",
                declaration.id().as_str(),
                observation.reason
            );
        }
        for evidence in &observation.tools {
            assert!(
                matches!(evidence, ToolEvidence::Identified { .. }),
                "{} lost a program the stub directory supplies",
                declaration.id().as_str()
            );
        }
    }

    measure_toolchain(observations).expect("version-drift observations are admitted");
}

#[test]
fn the_compiled_scanners_pass_over_a_clean_tree() {
    let repository = scanning_repository(
        "toolchain-clean",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[SOURCE, PROSE],
    );

    for id in SCANNING_ARMS {
        let observation = arm(&observations, id);
        assert_eq!(
            observation.outcome,
            ArmOutcome::Passed,
            "{} refused a clean tree: {}",
            id.as_str(),
            observation.reason
        );
        assert!(
            observation.findings.is_empty(),
            "{} passed while carrying {:?}",
            id.as_str(),
            observation.findings
        );
    }

    measure_toolchain(observations).expect("clean observations are admitted");
}

/// The fixture repeats one discouraged word and one path marker on one line.
///
/// Each pair carries the same detail on the same line of the same file, so the
/// column is the only field that tells the two apart. Admission refuses findings
/// that agree in every field, which makes these the cases that prove a second
/// occurrence survives as its own place to fix rather than collapsing into the
/// first.
#[test]
fn the_compiled_scanners_report_exact_findings_over_a_dirty_tree() {
    let (word, replacement) = discouraged_word();
    let prose = format!("This paragraph is {word} and {word} again.\n");
    let marked =
        format!("// {LOCAL_PATH_MARKER}somebody/devel and {LOCAL_PATH_MARKER}somebody/scratch\n");
    let repository = scanning_repository(
        "toolchain-dirty",
        &format!("{marked}fn main( ) {{\n    let  x=1;\n}}\n"),
        &prose,
    );
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[SOURCE, PROSE],
    );

    for id in GENERIC_FINDING_ARMS {
        let observation = arm(&observations, id);
        assert_eq!(
            observation.outcome,
            ArmOutcome::Findings,
            "{} admitted a dirty tree: {}",
            id.as_str(),
            observation.reason
        );
    }

    let formatting = &arm(&observations, GateId::Formatters).findings;
    assert_eq!(
        formatting,
        &[ArmFinding {
            path: Some(SOURCE.to_owned()),
            line: Some(1),
            column: None,
            detail: FindingDetail::Formatting {
                formatter: "rustfmt".to_owned(),
            },
        }],
        "the formatter named something other than the one unformatted file"
    );
    let local_path = |column| ArmFinding {
        path: Some(SOURCE.to_owned()),
        line: Some(1),
        column: Some(column),
        detail: FindingDetail::LocalPath,
    };
    assert_eq!(
        arm(&observations, GateId::LocalPaths).findings,
        [
            local_path(fixture_column(&marked, LOCAL_PATH_MARKER)),
            local_path(fixture_column(
                &marked,
                &format!("{LOCAL_PATH_MARKER}somebody/scratch")
            )),
        ],
        "the marker scanner named something other than the two marked occurrences"
    );
    let discouraged = |column| ArmFinding {
        path: Some(PROSE.to_owned()),
        line: Some(1),
        column: Some(column),
        detail: FindingDetail::Taste {
            word: word.clone(),
            replacement: replacement.clone(),
        },
    };
    assert_eq!(
        arm(&observations, GateId::Taste).findings,
        [
            discouraged(fixture_column(&prose, &word)),
            discouraged(fixture_column(&prose, &format!("{word} again"))),
        ],
        "the prose scanner named something other than the two discouraged occurrences"
    );

    measure_toolchain(observations).expect("finding observations are admitted");
}

/// What the compiled marker scanner emits, held against the release it replaces.
///
/// Every other parity case builds its current side from fixture findings, so
/// none of them observe a measurement the compiled scanners actually produced.
/// A marker alone on its line is one place to both producers and is where the
/// two must still agree; a doubled line is one place to the predecessor and
/// two here, which is the granularity the comparison holds apart. Pinning both
/// keeps a locus this scanner adds from changing what parity means unexamined.
#[test]
fn the_compiled_marker_scanner_compares_against_the_predecessor_it_replaces() {
    let doubled =
        format!("// {LOCAL_PATH_MARKER}somebody/devel and {LOCAL_PATH_MARKER}somebody/scratch\n");
    let cases = [
        (
            "toolchain-marker-parity-single",
            format!("// {LOCAL_PATH_MARKER}somebody/devel\n"),
            1,
            Ok(PredecessorComparison::Equivalent),
        ),
        (
            "toolchain-marker-parity-doubled",
            doubled,
            2,
            Err(PredecessorComparisonError::ResultMismatch(
                GateId::LocalPaths,
            )),
        ),
    ];

    for (label, marked, places, expected) in cases {
        let repository = scanning_repository(
            label,
            &format!("{marked}fn main() {{}}\n"),
            "Every sentence here is ordinary.\n",
        );
        let observations = acquired(
            &repository,
            Some(toolchain_directory().into_os_string()),
            &[SOURCE, PROSE],
        );
        let report = measure_toolchain(observations).expect("marker observations are admitted");
        let current = report
            .arms
            .iter()
            .find(|measurement| measurement.gate_id == GateId::LocalPaths)
            .expect("the marker scanner is measured");
        assert_eq!(
            current.findings.len(),
            places,
            "{label} named something other than the occurrences it marks"
        );
        assert!(
            current
                .findings
                .iter()
                .all(|finding| finding.column.is_some()),
            "{label} compared a column the compiled scanner never reported"
        );

        let predecessor = PredecessorObservation {
            gate_name: "local-paths".to_owned(),
            configuration: PredecessorConfiguration::PackagedDefaults,
            result: PredecessorResult::Findings(vec![PredecessorFinding::LocalPath {
                path: SOURCE.to_owned(),
                line: 1,
            }]),
        };
        assert_eq!(
            compare_predecessor(&predecessor, current),
            expected,
            "{label} compared against the release differently than the comparison documents"
        );
    }
}

#[test]
fn javascript_unused_reports_candidate_and_test_only_but_suppresses_the_exact_exemption() {
    let repository = scanning_repository(
        "toolchain-javascript-unused",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    let &(exempt_path, exempt_symbol, _) = PACKAGED_JAVASCRIPT_EXEMPTIONS
        .first()
        .expect("the packaged exemption fixture exists");
    repository.write(
        "web/exports.mjs",
        &format!(
            concat!(
                "export function unusedExport() {{}}\n",
                "export const testOnlyExport = 1;\n",
                "export function {}() {{}}\n",
            ),
            exempt_symbol,
        ),
    );
    repository.write(
        "tests/exports.spec.cjs",
        concat!(
            "import { testOnlyExport } from \"../web/exports.mjs\";\n",
            "console.log(testOnlyExport);\n",
        ),
    );
    repository.write(
        exempt_path,
        &format!("export function {exempt_symbol}() {{ return true; }}\n"),
    );
    git_commit(&repository, "javascript declaration fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::JavascriptUnused);
    assert_eq!(observation.outcome, ArmOutcome::Findings);
    assert_eq!(
        observation.findings,
        [
            ArmFinding {
                path: Some("web/exports.mjs".to_owned()),
                line: Some(1),
                column: None,
                detail: FindingDetail::JavascriptUnused {
                    symbol: "unusedExport".to_owned(),
                    declaration_kind: "function".to_owned(),
                    status: JavascriptUnusedStatus::CandidateUnused,
                    reason: "no_references_outside_declaration".to_owned(),
                    reference_count: 1,
                    test_reference_count: 0,
                },
            },
            ArmFinding {
                path: Some("web/exports.mjs".to_owned()),
                line: Some(2),
                column: None,
                detail: FindingDetail::JavascriptUnused {
                    symbol: "testOnlyExport".to_owned(),
                    declaration_kind: "constant".to_owned(),
                    status: JavascriptUnusedStatus::TestOnly,
                    reason: "references_only_in_tests".to_owned(),
                    reference_count: 1,
                    test_reference_count: 2,
                },
            },
            ArmFinding {
                path: Some("web/exports.mjs".to_owned()),
                line: Some(3),
                column: None,
                detail: FindingDetail::JavascriptUnused {
                    symbol: exempt_symbol.to_owned(),
                    declaration_kind: "function".to_owned(),
                    status: JavascriptUnusedStatus::CandidateUnused,
                    reason: "no_references_outside_declaration".to_owned(),
                    reference_count: 1,
                    test_reference_count: 0,
                },
            },
        ]
    );
    assert!(
        observation
            .findings
            .iter()
            .all(|finding| { finding.path.as_deref() != Some(exempt_path) })
    );

    measure_toolchain(observations).expect("JavaScript findings are admitted");
}

/// Four members go unreached here and only the three private ones are named.
///
/// The fourth is an interface member, which C# publishes through the interface
/// itself and which therefore carries no access modifier to say so. A member
/// that says nothing is otherwise private, so without the interface exception
/// the arm would claim a member the whole world may call.
#[test]
fn csharp_unused_reports_private_candidates_and_separates_what_policy_retains() {
    let repository = scanning_repository(
        "toolchain-csharp-unused",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write(
        "dotnet/Surface.cs",
        concat!(
            "public partial class Retained\n{\n",
            "    private void RetainedByPartial() { }\n}\n\n",
            "public class Surface\n{\n",
            "    [Obsolete]\n    private void AttributeRetained() { }\n\n",
            "    private void Loose() { }\n\n",
            "    private void Reached() { }\n\n",
            "    public void Entry()\n    {\n        Reached();\n    }\n}\n\n",
            "public interface IContract\n{\n    void Unreached();\n}\n",
        ),
    );
    git_commit(&repository, "csharp declaration fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::CsharpUnusedCandidates);
    assert_eq!(observation.outcome, ArmOutcome::Findings);
    let candidate = |symbol: &str, line| ArmFinding {
        path: Some("dotnet/Surface.cs".to_owned()),
        line: Some(line),
        column: None,
        detail: FindingDetail::CsharpUnused {
            symbol: symbol.to_owned(),
            declaration_kind: "method".to_owned(),
            status: CsharpUnusedStatus::CandidateUnused,
            reason: "no_references_outside_declaration".to_owned(),
            reference_count: 1,
        },
    };
    assert_eq!(
        observation.findings,
        [candidate("Loose", 11)],
        "the C# scanner named something other than the one member it will stand behind"
    );
    assert!(
        observation.reason.ends_with(
            "; 2 unreached members retained (attribute_retained 1, partial_declaration 1)"
        ),
        "the retained members left no trace: {}",
        observation.reason
    );

    measure_toolchain(observations).expect("C# findings are admitted");
}

/// An explicitly implemented interface member is not the class's to delete.
///
/// C# forbids an access modifier on an explicit implementation, so the silence
/// that otherwise means private means published here. The interface is declared
/// in a second file on purpose: references are counted per file, so the member's
/// own name is the only occurrence in the file that declares it, and an arm
/// reading silence as privacy accuses live contract code whose deletion would
/// not compile. `Loose` shares the file so the tree still proves the arm runs
/// and can accuse.
#[test]
fn csharp_unused_withdraws_an_explicit_interface_implementation() {
    let repository = scanning_repository(
        "toolchain-csharp-explicit",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write(
        "dotnet/IPersist.cs",
        "public interface IPersist\n{\n    void Save();\n}\n",
    );
    repository.write(
        "dotnet/Persister.cs",
        concat!(
            "public class Persister : IPersist\n{\n",
            "    void IPersist.Save() { }\n\n",
            "    private void Loose() { }\n}\n",
        ),
    );
    git_commit(&repository, "csharp explicit implementation fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::CsharpUnusedCandidates);
    let named: Vec<&str> = observation
        .findings
        .iter()
        .filter_map(|finding| match &finding.detail {
            FindingDetail::CsharpUnused { symbol, .. } => Some(symbol.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        named,
        ["Loose"],
        "the arm claimed a member the interface publishes"
    );

    measure_toolchain(observations).expect("C# findings are admitted");
}

/// A tree holding every unreached member live must not fail the battery.
///
/// An arm carries findings only under `ArmOutcome::Findings`, and that outcome
/// is the one route to `BatteryState::Failed`, so a retention that arrived as a
/// row would fail the gate on evidence that the member must be kept. The state
/// is read through the same mapping the battery uses rather than restated here.
#[test]
fn csharp_unused_retention_alone_leaves_the_battery_passing() {
    let repository = scanning_repository(
        "toolchain-csharp-retained",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write(
        "dotnet/Held.cs",
        concat!(
            "public partial class Held\n{\n",
            "    private void HeldByPartial() { }\n}\n\n",
            "public class Marked\n{\n",
            "    [Obsolete]\n    private void MarkedRetained() { }\n\n",
            "    private void Reached() { }\n\n",
            "    public void Entry()\n    {\n        Reached();\n    }\n}\n",
        ),
    );
    git_commit(&repository, "csharp retention fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::CsharpUnusedCandidates);
    assert_eq!(observation.outcome, ArmOutcome::Passed);
    assert!(observation.findings.is_empty());

    let report = measure_toolchain(observations).expect("retained-only C# evidence is admitted");
    let measured = report
        .arms
        .into_iter()
        .find(|measured| measured.gate_id == GateId::CsharpUnusedCandidates)
        .expect("the C# arm is measured");
    assert!(
        measured.reason.ends_with(
            "; 2 unreached members retained (attribute_retained 1, partial_declaration 1)"
        ),
        "the retained members left no trace: {}",
        measured.reason
    );
    let state = ArmEvidence::Toolchain { report: measured }
        .state()
        .expect("a toolchain arm states one battery state");
    assert_eq!(state, BatteryState::Passed);
}

/// A partial record holds its members the way a partial class does.
///
/// C# spells a record three ways and the grammar gives all three one node, so a
/// fixture naming only `record` would prove the node rather than the language.
/// Each form declares one private member no name reaches, and the count in the
/// reason is what proves every form was held: a form the arm cannot see encloses
/// nothing, so its member answers to no declaration and is accused instead.
#[test]
fn csharp_unused_retains_a_member_of_every_partial_record_form() {
    let repository = scanning_repository(
        "toolchain-csharp-record",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write(
        "dotnet/Records.cs",
        concat!(
            "public partial record Plain\n{\n",
            "    private void HeldByPlain() { }\n}\n\n",
            "public partial record class Classy\n{\n",
            "    private void HeldByClass() { }\n}\n\n",
            "public partial record struct Valued\n{\n",
            "    private void HeldByValue() { }\n}\n",
        ),
    );
    git_commit(&repository, "csharp partial record fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::CsharpUnusedCandidates);
    assert_eq!(observation.outcome, ArmOutcome::Passed);
    assert!(
        observation
            .reason
            .ends_with("; 3 unreached members retained (partial_declaration 3)"),
        "a partial record's members left no retained trace: {}",
        observation.reason
    );

    measure_toolchain(observations).expect("retained-only C# record evidence is admitted");
}

/// What a module owns, and what counts as reaching it.
///
/// The nested locals below are declared and never read, so an inventory that
/// admitted nested declarations would report both as unused. `shorthandUse` is
/// reached only through object shorthand and `sandboxSymbol` only through a
/// test harness namespace, so a scanner missing either vocabulary would report
/// that symbol as unused too. The production property access on the last line
/// names `bundle` without reaching it, because production code reaches a symbol
/// by name; counting it there would rescue a symbol nobody uses. The exact list
/// below is what survives all four.
#[test]
fn javascript_unused_scopes_candidates_to_top_level_and_counts_every_reference_form() {
    let repository = scanning_repository(
        "toolchain-javascript-scope",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write(
        "web/regression.mjs",
        concat!(
            "export function outer() {\n",
            "  const nestedLocal = 1;\n",
            "  function nestedHelper() {}\n",
            "  return 0;\n",
            "}\n",
            "const shorthandUse = 2;\n",
            "export const bundle = { shorthandUse };\n",
            "export function sandboxSymbol() {}\n",
            "outer();\n",
            "globalThis.bundle;\n",
        ),
    );
    repository.write("tests/regression.spec.cjs", "context.sandboxSymbol();\n");
    git_commit(&repository, "javascript scope fixture");

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );
    let observation = arm(&observations, GateId::JavascriptUnused);
    assert_eq!(observation.outcome, ArmOutcome::Findings);
    assert_eq!(
        observation.findings,
        [
            ArmFinding {
                path: Some("web/regression.mjs".to_owned()),
                line: Some(7),
                column: None,
                detail: FindingDetail::JavascriptUnused {
                    symbol: "bundle".to_owned(),
                    declaration_kind: "constant".to_owned(),
                    status: JavascriptUnusedStatus::CandidateUnused,
                    reason: "no_references_outside_declaration".to_owned(),
                    reference_count: 1,
                    test_reference_count: 0,
                },
            },
            ArmFinding {
                path: Some("web/regression.mjs".to_owned()),
                line: Some(8),
                column: None,
                detail: FindingDetail::JavascriptUnused {
                    symbol: "sandboxSymbol".to_owned(),
                    declaration_kind: "function".to_owned(),
                    status: JavascriptUnusedStatus::TestOnly,
                    reason: "references_only_in_tests".to_owned(),
                    reference_count: 1,
                    test_reference_count: 1,
                },
            },
        ]
    );

    measure_toolchain(observations).expect("JavaScript findings are admitted");
}

/// The formatter gap `cargo fmt --all` cannot see, proven from both sides.
///
/// The declared workspace pass is run first over the same tree and has to
/// report nothing, so the finding below is attributable to the second pass
/// rather than to a fixture that was unformatted somewhere the first pass could
/// already reach. Splitting a file off with `include!` is not a hypothetical
/// shape here: seven files still reach a sibling that way, `proof/src/container.rs`
/// among them, and every one of them was drifting unread until this pass existed.
#[test]
fn the_formatter_names_a_file_only_the_included_pass_can_reach() {
    let repository = including_repository("toolchain-included", UNFORMATTED_INCLUDED);
    let workspace = workspace_formatter_output(&repository.root);
    assert!(
        !workspace.contains("Diff in"),
        "the declared workspace pass already reached the included file, so this \
         fixture cannot prove the gap it exists to prove: {workspace}"
    );

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[INCLUDED],
    );

    assert_eq!(
        arm(&observations, GateId::Formatters).findings,
        [ArmFinding {
            path: Some(INCLUDED.to_owned()),
            line: Some(1),
            column: None,
            detail: FindingDetail::Formatting {
                formatter: "rustfmt".to_owned(),
            },
        }],
        "the formatter arm named something other than the one file the \
         workspace pass cannot open"
    );
}

/// A second pass that cannot read its file refuses, and never reports clean.
///
/// This is the failure mode the gap itself was made of: the arm concluding a
/// tree is formatted because nothing told it otherwise. An included file no
/// edition parses leaves the second pass exiting nonzero with no file named,
/// and the workspace pass -- which never opens that file -- stays silent, so
/// findings alone would read exactly like a canonical tree.
#[test]
fn a_second_pass_that_names_no_file_refuses_instead_of_passing() {
    let repository = including_repository("toolchain-unparsable", UNPARSABLE_INCLUDED);
    let workspace = workspace_formatter_output(&repository.root);
    assert!(
        !workspace.contains("Diff in"),
        "the declared workspace pass reached the unparsable file, so its silence \
         is not what leaves the arm with nothing to report: {workspace}"
    );

    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[INCLUDED],
    );

    let formatters = arm(&observations, GateId::Formatters);
    assert_eq!(
        formatters.outcome,
        ArmOutcome::NonzeroAnalysis,
        "the formatter arm reported {:?} over a file it could not read",
        formatters.reason
    );
    assert!(
        formatters.findings.is_empty(),
        "a refusing pass named a differing file it never reported"
    );
    assert!(
        formatters.reason.starts_with("rustfmt refused with "),
        "the refusal named something other than the pass that earned it: {}",
        formatters.reason
    );
}
