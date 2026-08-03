use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{
    ArmFinding, ArmObservation, ArmOutcome, Contract, EnvironmentLedgerKind, FindingDetail, GateId,
    LOCAL_PATH_MARKER, PACKAGED_ENV_PROFILE, PACKAGED_TASTE_WORDS, Selector, SourceLocus,
    TOOLCHAIN_ARMS, TOOLCHAIN_REPORT_VERSION, ToolEvidence, ToolchainSource, acquire_toolchain,
    measure_toolchain,
};
use wire::{Digest, Domain};

mod support;

use support::{Tree, contract_repository, git_commit};

/// The version every stub reports when it is asked to be wrong.
const DRIFTED_VERSION: &str = "cargo 0.0.0 (0000000000 1970-01-01)";

/// The program the drift fixture makes wrong; every other stub stays exact.
const DRIFTED_PROGRAM: &str = "cargo";

/// The arms this build analyzes itself rather than owing an analyzer.
const COMPILED_ARMS: [GateId; 6] = [
    GateId::Formatters,
    GateId::LocalPaths,
    GateId::Taste,
    GateId::CanvasWebTypecheck,
    GateId::EnvPolicy,
    GateId::EnvNameLedger,
];

/// The compiled scanners that are applicable to the shared source fixture.
///
/// The environment arms are compiled too, and a Rust source names no
/// environment, so the shared fixture leaves them nothing to find and they are
/// held to that in their own fixtures rather than here.
const SCANNING_ARMS: [GateId; 3] = [GateId::Formatters, GateId::LocalPaths, GateId::Taste];

/// The prose file both scanning fixtures carry, clean in one and dirty in the other.
const PROSE: &str = "docs/guide.md";

/// The Rust file the formatter reads and the marker scanner searches.
const SOURCE: &str = "src/main.rs";

/// The Python file carrying one waived access and one unwaived access.
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

#[test]
fn the_compiled_scanners_report_exact_findings_over_a_dirty_tree() {
    let (word, replacement) = discouraged_word();
    let repository = scanning_repository(
        "toolchain-dirty",
        &format!("// {LOCAL_PATH_MARKER}somebody/devel\nfn main( ) {{\n    let  x=1;\n}}\n"),
        &format!("This paragraph is {word} and nothing else.\n"),
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
    assert_eq!(
        arm(&observations, GateId::LocalPaths).findings,
        [ArmFinding {
            path: Some(SOURCE.to_owned()),
            line: Some(1),
            column: None,
            detail: FindingDetail::LocalPath,
        }],
        "the marker scanner named something other than the one marked line"
    );
    assert_eq!(
        arm(&observations, GateId::Taste).findings,
        [ArmFinding {
            path: Some(PROSE.to_owned()),
            line: Some(1),
            column: None,
            detail: FindingDetail::Taste { word, replacement },
        }],
        "the prose scanner named something other than the one discouraged word"
    );

    measure_toolchain(observations).expect("finding observations are admitted");
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
    let source = ToolchainSource::capture(&tree, search_path, selected.clone());
    assert_eq!(
        source.root(),
        tree.root(),
        "acquisition observes the captured tree and not the live repository"
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
                ToolEvidence::Missing { name, .. } | ToolEvidence::Identified { name, .. } => name,
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
    repository.write(
        SETTINGS,
        &format!("import os\n\nhome = os.getenv(\"{WAIVED_NAME}\")  # {marker}\n{READ}\n"),
    );
    repository.write(WORKER, &format!("import os\n\n{READ}\n"));
    git_commit(&repository, "fixture environment sources");
    repository
}

/// One waived access and one unwaived access are one finding, not two.
///
/// The waiver is the whole point of the arm: an access is admitted by being
/// written down, so an arm that reported the annotated read too would make the
/// inventory unreadable and an arm that reported neither would make it empty.
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
    let column = READ.find(SHARED_NAME).expect("the read names the literal") + 1;
    assert_eq!(
        observation.findings,
        [ArmFinding {
            path: Some(SETTINGS.to_owned()),
            line: Some(4),
            column: Some(u32::try_from(column).expect("a fixture column fits")),
            detail: FindingDetail::EnvironmentAccess {
                name: SHARED_NAME.to_owned(),
                source: READ.to_owned(),
            },
        }],
        "the policy arm named something other than the one unwaived access"
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
    String::from_utf8(output.stdout)
        .expect("version probes print UTF-8")
        .trim_end_matches(['\n', '\r'])
        .to_owned()
}
