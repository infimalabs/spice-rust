use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{
    ArmEvidence, ArmFinding, ArmObservation, ArmOutcome, BatteryState, Contract,
    CsharpUnusedStatus, EnvironmentLedgerKind, FindingDetail, GateId, JavascriptUnusedStatus,
    LOCAL_PATH_MARKER, PACKAGED_ENV_PROFILE, PACKAGED_JAVASCRIPT_EXEMPTIONS, PACKAGED_TASTE_WORDS,
    PredecessorComparison, PredecessorComparisonError, PredecessorConfiguration,
    PredecessorFinding, PredecessorObservation, PredecessorResult, ProbeFailure, RimCapture,
    Selector, SourceLocus, TOOLCHAIN_ARMS, TOOLCHAIN_REPORT_VERSION, ToolEvidence, ToolchainSource,
    acquire_toolchain, compare_predecessor, measure_toolchain,
};
use wire::{Digest, Domain};

mod support;

use support::{Tree, contract_repository, git_commit};

/// The version every stub reports when it is asked to be wrong.
const DRIFTED_VERSION: &str = "cargo 0.0.0 (0000000000 1970-01-01)";

/// The program the drift fixture makes wrong; every other stub stays exact.
const DRIFTED_PROGRAM: &str = "cargo";

/// The status every stub exits with when it is asked to refuse its probe.
const REFUSING_STATUS: i32 = 41;

/// The arms this build analyzes itself rather than owing an analyzer.
const COMPILED_ARMS: [GateId; 8] = [
    GateId::Formatters,
    GateId::LocalPaths,
    GateId::Taste,
    GateId::JavascriptUnused,
    GateId::CsharpUnusedCandidates,
    GateId::CanvasWebTypecheck,
    GateId::EnvPolicy,
    GateId::EnvNameLedger,
];

/// The compiled scanners applicable to every shared source fixture.
///
/// The environment arms are compiled too, and neither a Rust source nor a clean
/// JavaScript module names an environment, so the shared fixture leaves them
/// nothing to find and they are held to that in their own fixtures rather than
/// here.
const SCANNING_ARMS: [GateId; 4] = [
    GateId::Formatters,
    GateId::LocalPaths,
    GateId::Taste,
    GateId::JavascriptUnused,
];

/// The three compiled scanners the generic dirty fixture intentionally violates.
const GENERIC_FINDING_ARMS: [GateId; 3] = [GateId::Formatters, GateId::LocalPaths, GateId::Taste];

/// The prose file both scanning fixtures carry, clean in one and dirty in the other.
const PROSE: &str = "docs/guide.md";

/// The Rust file the formatter reads and the marker scanner searches.
const SOURCE: &str = "src/main.rs";

/// The Python file carrying one waived access and three unwaived lines.
const SETTINGS: &str = "app/settings.py";

/// The second Python file reading the same environment name.
const WORKER: &str = "app/worker.py";

/// The environment name only the waived access reads.
///
/// Both fixture names are drawn from the names this repository already declares
/// in `[policy] env_names`. A fixture literal is a real reference as far as this
/// repository's own environment ledger is concerned, so an invented name here
/// would demand a manifest entry for a name nothing reads.
const WAIVED_NAME: &str = "SPICE_HOOKS"; // env-policy: allow

/// The environment name both fixture files read without a waiver.
const SHARED_NAME: &str = "SPICE_LANE"; // env-policy: allow

/// The exact unwaived read both fixture files spell.
const READ: &str = "cache = os.getenv(\"SPICE_LANE\")";

/// One line reading a watchlisted name beside a name nobody wrote down.
///
/// Neither extractor sees this line whole: the watchlist names the first read
/// and the access gate is the only thing that catches the second.
const MIXED: &str = "flag = os.getenv(\"SPICE_LANE\") or os.environ[key]";

/// One line committing two reads under names nobody wrote down.
///
/// Both accesses report under the same family idiom on the same line of the
/// same file, so the column is the only thing that keeps the second from
/// arriving as a duplicate of the first and being refused.
const DYNAMIC: &str = "both = os.environ[first] or os.getenv(second)";

/// The clean JavaScript module that makes the complete-tree analysis applicable.
const JAVASCRIPT: &str = "web/app.js";

/// The clean C# unit that makes the complete-tree analysis applicable.
const CSHARP: &str = "dotnet/App.cs";

/// One C# type whose sole member is reached from inside the same unit.
///
/// The declaration alone is one occurrence, so a member the fixture never calls
/// would report as a candidate. Calling it once is what makes this tree clean
/// without exempting anything.
const CSHARP_SOURCE: &str = concat!(
    "public class App\n{\n",
    "    public void Run()\n    {\n        Step();\n    }\n\n",
    "    public void Step()\n    {\n    }\n}\n"
);

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

/// A refusal the formatter cannot explain is retained with the code it gave.
///
/// [`measure_toolchain`] admits an exit code only beside `NonzeroAnalysis` and
/// demands one there, so an analysis that reports the outcome without the code
/// is refused at admission rather than at the call. The captured tree here has
/// Rust sources and no manifest, which is a refusal about the tree rather than
/// about any one file, so the formatter names no differing file.
///
/// The code ships twice, once as a field and once inside the sentence, and both
/// spellings are held to each other. A reader shown a sentence naming one code
/// beside a field carrying another has been handed an invented number.
#[test]
fn a_formatter_refusal_naming_no_file_carries_the_code_it_refused_with() {
    let repository = contract_repository("toolchain-unreadable");
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );

    let observation = arm(&observations, GateId::Formatters);
    assert_eq!(
        observation.outcome,
        ArmOutcome::NonzeroAnalysis,
        "the formatter read a tree Cargo has no manifest for: {}",
        observation.reason
    );
    let code = observation
        .exit_code
        .expect("a refusal carries the code it refused with");
    assert_ne!(code, 0, "zero is not a code that explains a refusal");
    assert!(
        observation
            .reason
            .contains(&format!("refused with {code} ")),
        "the field carries {code} and the sentence names another: {}",
        observation.reason
    );
    assert!(
        observation.findings.is_empty(),
        "an unexplained refusal invented {:?}",
        observation.findings
    );

    measure_toolchain(observations).expect("an explained refusal is admitted");
}

#[test]
fn canvas_public_acquisition_retains_unparsed_nonzero_without_running_trunk() {
    let repository = scanning_repository(
        "toolchain-canvas-nonzero",
        "fn main() {}\n",
        "Every sentence here is ordinary.\n",
    );
    repository.write("app/index.html", "<main></main>\n");
    git_commit(&repository, "canvas entrypoint");
    let stubs = canvas_nonzero_directory("toolchain-canvas-tools");
    let observations = acquired(&repository, Some(stubs.root.clone().into_os_string()), &[]);

    let observation = arm(&observations, GateId::CanvasWebTypecheck);
    assert_eq!(observation.outcome, ArmOutcome::NonzeroAnalysis);
    assert_eq!(observation.exit_code, Some(17));
    assert!(observation.findings.is_empty());
    assert!(
        observation
            .reason
            .contains("cargo wasm32 check refused with 17")
    );
    assert!(
        !stubs.path("trunk-analysis-ran").exists(),
        "trunk ran after the cargo analysis refused"
    );

    measure_toolchain(observations).expect("canvas nonzero observation is admitted");
    repository.remove();
    stubs.remove();
}

/// Acquire one battery over a committed fixture tree and a captured search path.
///
/// Every invariant that holds for all three fixtures is asserted here once, so
/// each test states only what makes it different.
fn acquired(
    repository: &Tree,
    search_path: Option<OsString>,
    changes: &[&str],
) -> Vec<ArmObservation> {
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let selected: Vec<String> = changes.iter().map(|path| (*path).to_owned()).collect();
    let source = ToolchainSource::capture(
        &tree,
        RimCapture::new(&repository.root, search_path, selected.clone())
            .expect("the fixture repository root is absolute"),
    );
    assert_eq!(
        source.root(),
        tree.root(),
        "acquisition observes the captured tree and not the live repository"
    );
    assert_eq!(
        source.rim().directory(),
        repository.root,
        "acquisition anchors the search path where the rim captured it"
    );
    assert_eq!(
        source.changes(),
        selected,
        "acquisition scans the changes the rim captured and no others"
    );

    let observations = acquire_toolchain(&source, &contract);
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.gate_id)
            .collect::<Vec<_>>(),
        TOOLCHAIN_ARMS
            .into_iter()
            .map(|declaration| declaration.id())
            .collect::<Vec<_>>(),
        "one observation per declared arm, in declaration order, none invented"
    );

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        assert_ne!(
            observation.outcome,
            ArmOutcome::Excluded,
            "{} answered an exclusion acquisition never measured",
            declaration.id().as_str()
        );
        if COMPILED_ARMS.contains(&declaration.id()) {
            assert_ne!(
                observation.outcome,
                ArmOutcome::Unsupported,
                "{} has a compiled analysis and still called itself unanalyzed",
                declaration.id().as_str()
            );
        } else {
            assert!(
                matches!(
                    observation.outcome,
                    ArmOutcome::Unsupported
                        | ArmOutcome::ToolUnavailable
                        | ArmOutcome::UnsupportedVersion
                ),
                "{} has no compiled analysis and answered {:?} anyway",
                declaration.id().as_str(),
                observation.outcome
            );
            assert!(
                observation.findings.is_empty(),
                "{} carried findings no compiled analysis produced",
                declaration.id().as_str()
            );
        }
        assert!(
            !observation.reason.is_empty(),
            "{} reported no cause",
            declaration.id().as_str()
        );
        assert_eq!(
            observation.tools.len(),
            declaration.tools().len(),
            "{} dropped or invented an evidence row",
            declaration.id().as_str()
        );
        for (requirement, evidence) in declaration.tools().iter().zip(&observation.tools) {
            let name = match evidence {
                ToolEvidence::Missing { name, .. }
                | ToolEvidence::Unprobed { name, .. }
                | ToolEvidence::Identified { name, .. } => name,
            };
            assert_eq!(
                name,
                requirement.name(),
                "{} carried its evidence out of declaration order",
                declaration.id().as_str()
            );
        }
    }
    observations
}

/// One committed fixture repository Cargo can read and the scanners can scan.
///
/// Formatters runs the declared `cargo fmt --all -- --check` for real, so the
/// captured tree has to be a package Cargo will actually open.
fn scanning_repository(label: &str, source: &str, prose: &str) -> Tree {
    let repository = contract_repository(label);
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    repository.write(SOURCE, source);
    repository.write(PROSE, prose);
    repository.write(JAVASCRIPT, "void 0;\n");
    repository.write(CSHARP, CSHARP_SOURCE);
    git_commit(&repository, "fixture sources");
    repository
}

/// One committed fixture repository whose Python names the environment.
///
/// One access carries the waiver and one does not, and one name is read in two
/// files, so each environment arm has exactly one thing to say about it.
fn environment_repository(label: &str) -> Tree {
    let repository = contract_repository(label);
    repository.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    let marker = PACKAGED_ENV_PROFILE.allow_marker;
    let waived = format!("home = os.getenv(\"{WAIVED_NAME}\")  # {marker}");
    repository.write(
        SETTINGS,
        &format!("import os\n\n{waived}\n{READ}\n{MIXED}\n{DYNAMIC}\n"),
    );
    repository.write(WORKER, &format!("import os\n\n{READ}\n"));
    git_commit(&repository, "fixture environment sources");
    repository
}

/// Every unwaived access is one finding, and the waived one is none.
///
/// The waiver is the whole point of the arm: an access is admitted by being
/// written down, so an arm that reported the annotated read too would make the
/// inventory unreadable and an arm that reported neither would make it empty.
/// Past the waiver this holds the arm to one finding per access rather than per
/// line, over a line whose two reads only one extractor each can name and a
/// line whose two reads share a name and differ by column alone. Admission
/// refuses findings that agree in every field, so the second of those two is
/// retained only because the column tells it apart.
#[test]
fn the_environment_policy_arm_reports_only_the_unwaived_access() {
    let repository = environment_repository("toolchain-env-policy");
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[SETTINGS],
    );

    let observation = arm(&observations, GateId::EnvPolicy);
    assert_eq!(
        observation.outcome,
        ArmOutcome::Findings,
        "the policy arm read a tree with one unwaived access: {}",
        observation.reason
    );
    let idiom = python_idiom();
    let access = |line: u32, source: &str, needle: &str, name: &str| ArmFinding {
        path: Some(SETTINGS.to_owned()),
        line: Some(line),
        column: Some(fixture_column(source, needle)),
        detail: FindingDetail::EnvironmentAccess {
            name: name.to_owned(),
            source: source.to_owned(),
        },
    };
    assert_eq!(
        observation.findings,
        [
            access(4, READ, SHARED_NAME, SHARED_NAME),
            access(5, MIXED, SHARED_NAME, SHARED_NAME),
            access(5, MIXED, "os.environ", &idiom),
            access(6, DYNAMIC, "os.environ", &idiom),
            access(6, DYNAMIC, "os.getenv", &idiom),
        ],
        "the policy arm named something other than the five unwaived accesses"
    );

    measure_toolchain(observations).expect("environment observations are admitted");
}

/// A name the profile never declared is unaccounted at every locus reading it.
///
/// The ledger is repository-wide, so it carries loci instead of a locus, and
/// dropping either one would understate where the environment shapes behavior.
/// Nothing was selected as changed here, which is also what proves the policy
/// arm reaches `NotApplicable` only through its own declared predicate.
#[test]
fn the_environment_ledger_arm_accounts_an_unaccounted_name_at_every_locus() {
    let repository = environment_repository("toolchain-env-ledger");
    let observations = acquired(
        &repository,
        Some(toolchain_directory().into_os_string()),
        &[],
    );

    let observation = arm(&observations, GateId::EnvNameLedger);
    assert_eq!(
        observation.outcome,
        ArmOutcome::Findings,
        "the ledger arm read a tree declaring none of the names it uses: {}",
        observation.reason
    );
    let accounted = observation
        .findings
        .iter()
        .find(|finding| match &finding.detail {
            FindingDetail::EnvironmentLedger { name, .. } => name == SHARED_NAME,
            _ => false,
        })
        .unwrap_or_else(|| panic!("the ledger never accounted {SHARED_NAME}"));
    assert_eq!(
        (accounted.path.as_deref(), accounted.line, accounted.column),
        (None, None, None),
        "a ledger finding is repository-wide and carries no locus of its own"
    );
    let FindingDetail::EnvironmentLedger {
        mismatch,
        references,
        ..
    } = &accounted.detail
    else {
        panic!("the accounted finding stopped being a ledger finding");
    };
    assert_eq!(*mismatch, EnvironmentLedgerKind::Unaccounted);
    assert_eq!(
        references,
        &[
            SourceLocus {
                path: SETTINGS.to_owned(),
                line: 4,
            },
            SourceLocus {
                path: SETTINGS.to_owned(),
                line: 5,
            },
            SourceLocus {
                path: WORKER.to_owned(),
                line: 3,
            },
        ]
    );

    let policy = arm(&observations, GateId::EnvPolicy);
    assert_eq!(
        policy.outcome,
        ArmOutcome::NotApplicable,
        "the policy arm skipped a tree for a reason other than its predicate: {}",
        policy.reason
    );
    let applicability = TOOLCHAIN_ARMS
        .into_iter()
        .find(|declaration| declaration.id() == GateId::EnvPolicy)
        .expect("env-policy is declared")
        .jurisdiction()
        .applicability();
    assert!(
        policy.reason.contains(applicability),
        "the skip named something other than the declared predicate: {}",
        policy.reason
    );

    measure_toolchain(observations).expect("environment observations are admitted");
}

/// The one-based character column one needle occupies in one fixture line.
fn fixture_column(line: &str, needle: &str) -> u32 {
    let start = line
        .find(needle)
        .expect("the fixture line names the needle");
    u32::try_from(line[..start].chars().count() + 1).expect("a fixture column fits")
}

/// The stable finding name the profile gives a dynamic Python access.
///
/// Read rather than spelled, so the fixture cannot drift from the compiled
/// profile the arm actually reports under.
fn python_idiom() -> String {
    PACKAGED_ENV_PROFILE
        .finding_names
        .iter()
        .find(|(family, _)| *family == "python")
        .map(|(_, name)| (*name).to_owned())
        .expect("the packaged profile names the python access family")
}

/// The one observation carrying this identity.
fn arm(observations: &[ArmObservation], id: GateId) -> &ArmObservation {
    observations
        .iter()
        .find(|observation| observation.gate_id == id)
        .unwrap_or_else(|| panic!("{} was not observed", id.as_str()))
}

/// The first discouraged word and its replacement, read from the vocabulary.
///
/// Read rather than spelled so the fixture cannot drift from the compiled
/// vocabulary, and so this file never has to contain a discouraged word.
fn discouraged_word() -> (String, String) {
    let (pattern, replacement) = PACKAGED_TASTE_WORDS
        .first()
        .expect("the compiled vocabulary is not empty");
    (
        pattern.trim_end_matches('*').to_owned(),
        (*replacement).to_owned(),
    )
}

/// The directory holding the Cargo that built this test.
fn toolchain_directory() -> PathBuf {
    Path::new(env!("CARGO"))
        .parent()
        .expect("the building Cargo sits in a directory")
        .to_owned()
}

/// One directory supplying every declared program, with exactly one wrong.
fn stub_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    let mut declared = BTreeMap::new();
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            declared.insert(requirement.program(), requirement.exact_version());
        }
    }
    assert!(
        declared.contains_key(DRIFTED_PROGRAM),
        "the drift fixture names a program no declaration requires"
    );
    declared.insert(DRIFTED_PROGRAM, DRIFTED_VERSION);

    for (program, version) in declared {
        assert!(
            !version.contains('\''),
            "{program} declares a version this stub cannot quote"
        );
        stubs.write(program, &format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"));
        let path = stubs.path(program);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
    }
    stubs
}

/// One directory supplying every declared program, each refusing its probe.
///
/// The programs are present and executable, so nothing here is missing. They
/// simply never answer the question their declaration asks them.
fn refusing_stub_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            stubs.write(
                requirement.program(),
                &format!("#!/bin/sh\nexit {REFUSING_STATUS}\n"),
            );
            let path = stubs.path(requirement.program());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
        }
    }
    stubs
}

/// Exact-version stubs whose cargo analysis refuses before trunk can run.
fn canvas_nonzero_directory(label: &str) -> Tree {
    let stubs = Tree::new(label);
    let mut declared = BTreeMap::new();
    for declaration in TOOLCHAIN_ARMS {
        for requirement in declaration.tools() {
            declared.insert(requirement.program(), requirement.exact_version());
        }
    }

    for (program, version) in declared {
        assert!(
            !version.contains('\''),
            "{program} version must be shell safe"
        );
        let body = match program {
            "cargo" => format!(
                concat!(
                    "#!/bin/sh\n",
                    "if [ \"$1\" = \"--version\" ]; then printf '%s\\n' '{version}'; exit 0; fi\n",
                    "if [ \"$1\" = \"fmt\" ]; then exit 0; fi\n",
                    "if [ \"$1\" = \"check\" ]; then printf 'opaque refusal\\n' >&2; exit 17; fi\n",
                    "exit 91\n",
                ),
                version = version,
            ),
            "trunk" => {
                let marker = stubs.path("trunk-analysis-ran");
                assert!(
                    !marker.to_string_lossy().contains('\''),
                    "trunk marker path must be shell safe"
                );
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "if [ \"$1\" = \"--version\" ]; then ",
                        "printf '%s\\n' '{version}'; exit 0; fi\n",
                        ": > '{marker}'\n",
                        "exit 99\n",
                    ),
                    version = version,
                    marker = marker.display(),
                )
            }
            _ => format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"),
        };
        stubs.write(program, &body);
        let path = stubs.path(program);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
    }
    stubs
}

/// Ask one resolved program for its version without going through the gate.
///
/// Nothing is trimmed, because the point of the comparison this feeds is that
/// acquisition retained the bytes rather than a cleaned-up rendering of them.
fn probe_directly(program: &Path, arguments: &[&str]) -> String {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("probe {}: {error}", program.display()));
    assert!(
        output.status.success(),
        "{} refused its version probe",
        program.display()
    );
    String::from_utf8(output.stdout).expect("version probes print UTF-8")
}
