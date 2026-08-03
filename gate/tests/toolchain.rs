use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use gate::{
    ArmObservation, ArmOutcome, Contract, Selector, TOOLCHAIN_ARMS, TOOLCHAIN_REPORT_VERSION,
    ToolEvidence, ToolchainSource, acquire_toolchain, measure_toolchain,
};
use wire::{Digest, Domain};

mod support;

use support::{Tree, contract_repository};

/// The version every stub reports when it is asked to be wrong.
const DRIFTED_VERSION: &str = "cargo 0.0.0 (0000000000 1970-01-01)";

/// The program the drift fixture makes wrong; every other stub stays exact.
const DRIFTED_PROGRAM: &str = "cargo";

#[test]
fn acquisition_over_a_real_tree_yields_a_report_measure_toolchain_admits() {
    let repository = contract_repository("toolchain-real");
    let search_path = toolchain_directory();
    let observations = acquired(&repository, Some(search_path.clone().into_os_string()));

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
    let repository = contract_repository("toolchain-absent");
    let empty = Tree::new("toolchain-empty-path");
    let observations = acquired(&repository, Some(empty.root.clone().into_os_string()));

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        if declaration.tools().is_empty() {
            assert_eq!(
                observation.outcome,
                ArmOutcome::Unsupported,
                "{} declares no tool, so nothing can be unavailable",
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
    let repository = contract_repository("toolchain-drift");
    let stubs = stub_directory("toolchain-drifted-cargo");
    let observations = acquired(&repository, Some(stubs.root.clone().into_os_string()));

    for (declaration, observation) in TOOLCHAIN_ARMS.into_iter().zip(&observations) {
        let drifted = declaration
            .tools()
            .iter()
            .any(|requirement| requirement.program() == DRIFTED_PROGRAM);
        let expected = if drifted {
            ArmOutcome::UnsupportedVersion
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

/// Acquire one battery over a committed fixture tree and a captured search path.
///
/// Every invariant that holds for all three fixtures is asserted here once, so
/// each test states only what makes it different.
fn acquired(repository: &Tree, search_path: Option<OsString>) -> Vec<ArmObservation> {
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let source = ToolchainSource::capture(&tree, search_path);
    assert_eq!(
        source.root(),
        tree.root(),
        "acquisition observes the captured tree and not the live repository"
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
        assert!(
            !matches!(
                observation.outcome,
                ArmOutcome::NotApplicable | ArmOutcome::Excluded
            ),
            "{} answered a skip acquisition never measured",
            declaration.id().as_str()
        );
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
