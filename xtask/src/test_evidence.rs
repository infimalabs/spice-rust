//! Production publication of the shared long-running test-evidence graph.

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::Path;

#[cfg(test)]
use gate::MutationExecution;
use gate::{TestEvidenceGraph, TestEvidenceReport};
use sill::Environment;
use spice::ambient::Ambient;

use crate::workspace_tests;

const COVERAGE_UNKNOWN: &str = concat!(
    "the canonical workspace-test execution published its complete target inventory, ",
    "but no authoritative per-test covered-source denominator"
);
const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";
const CARGO_BUILD_BUILD_DIR: &str = "CARGO_BUILD_BUILD_DIR";

/// Capture one staged source, run both canonical executors, and publish all folds.
///
/// Workspace-test progress is emitted separately from the single JSON report so
/// the report remains machine-readable. The selected-source capture stays alive
/// across both executions. Until the canonical test runner gains a trustworthy
/// per-test coverage publisher, it declares coverage unknown and the blocking
/// folds refuse instead of manufacturing a clean denominator.
///
/// # Errors
///
/// Returns repository discovery, source capture, workspace-test, mutation,
/// graph-construction, report-encoding, or output failures.
pub fn run(
    ambient: &Ambient,
    progress: &mut impl Write,
    output: &mut impl Write,
) -> Result<TestEvidenceReport, TestEvidenceRunError> {
    let repository = spice::repository::Repository::discover(Path::new("."))
        .map_err(TestEvidenceRunError::new)?;
    let environment = ambient.environment().map_err(TestEvidenceRunError::new)?;
    let environment = selected_test_environment(environment, repository.worktree_root())?;
    let prepared = spice::gate::prepare_strict(ambient, gate::Selector::staged())
        .map_err(TestEvidenceRunError::new)?;
    let workspace = workspace_tests::run_at(
        prepared.root(),
        environment,
        gate::WORKSPACE_TEST_CONCURRENCY_CAP,
        progress,
    )
    .map_err(TestEvidenceRunError::new)?;
    let kill = prepared.execute_kill().map_err(TestEvidenceRunError::new)?;
    let tests = workspace
        .publish_unknown(
            prepared.source().clone(),
            prepared.changes().to_vec(),
            COVERAGE_UNKNOWN,
        )
        .map_err(TestEvidenceRunError::new)?;
    let report = TestEvidenceGraph::from_kill(tests, &kill, Vec::new())
        .map(|graph| graph.report())
        .map_err(TestEvidenceRunError::new)?;
    let document = report.encode().map_err(TestEvidenceRunError::new)?;
    output
        .write_all(&document)
        .and_then(|()| output.write_all(b"\n"))
        .map_err(TestEvidenceRunError::new)?;
    Ok(report)
}

#[cfg(test)]
fn joined_report(
    tests: gate::CanonicalTestExecution,
    mutation: MutationExecution,
) -> Result<TestEvidenceReport, TestEvidenceRunError> {
    TestEvidenceGraph::new(tests, mutation, Vec::new())
        .map(|graph| graph.report())
        .map_err(TestEvidenceRunError::new)
}

fn selected_test_environment(
    environment: Environment,
    repository: &Path,
) -> Result<Environment, TestEvidenceRunError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    let build_root = repository.join("target/test-evidence");
    entries.insert(
        OsString::from(CARGO_TARGET_DIR),
        build_root.join("cargo-target").into_os_string(),
    );
    entries.insert(
        OsString::from(CARGO_BUILD_BUILD_DIR),
        build_root.join("cargo-build").into_os_string(),
    );
    Environment::new(entries).map_err(TestEvidenceRunError::new)
}

/// One refusal from the production test-evidence authority.
#[derive(Debug)]
pub struct TestEvidenceRunError {
    detail: String,
}

impl TestEvidenceRunError {
    fn new(error: impl fmt::Display) -> Self {
        Self {
            detail: error.to_string(),
        }
    }
}

impl fmt::Display for TestEvidenceRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "test-evidence publication refused: {}",
            self.detail
        )
    }
}

impl Error for TestEvidenceRunError {}

#[cfg(test)]
mod tests {
    use super::*;
    use gate::{
        CanonicalTestExecution, CoverageCompleteness, MutantDisposition, MutationObservation,
        SurvivorRate, TestEvidenceStanding,
    };
    use std::ffi::OsStr;
    use wire::{Digest, Domain};

    fn source(label: &[u8]) -> Digest {
        Digest::compute(Domain::GateTree, label)
    }

    fn mutation(source: Digest) -> MutationExecution {
        MutationExecution::new(
            source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            vec![
                MutationObservation::new(
                    "mutant src/lib.rs:9 replace + with -",
                    "src/lib.rs",
                    MutantDisposition::ConfirmedKilled,
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn one_join_publishes_every_unknown_decision_and_exact_provenance() {
        let selected = source(b"one selected source");
        let tests = CanonicalTestExecution::new(
            selected.clone(),
            vec!["src/lib.rs".to_owned()],
            vec!["gate::lib".to_owned()],
            Vec::new(),
            CoverageCompleteness::unknown(COVERAGE_UNKNOWN).unwrap(),
        )
        .unwrap();

        let report = joined_report(tests, mutation(selected.clone())).unwrap();
        assert_eq!(report.source(), &selected);
        assert_eq!(report.tests().tests_run(), ["gate::lib"]);
        assert_eq!(report.mutation().mutants()[0].source(), "src/lib.rs");
        assert_eq!(
            report.suite_seam().standing(),
            TestEvidenceStanding::Unknown
        );
        assert_eq!(
            report.mutation_coverage().standing(),
            TestEvidenceStanding::Unknown
        );
        assert_eq!(
            report.test_subsumption().standing(),
            TestEvidenceStanding::Unknown
        );
        assert_eq!(
            report.instrument_admits(gate::GateId::SuiteSeam),
            Some(false)
        );
        assert_eq!(
            report.instrument_admits(gate::GateId::MutationCoverage),
            Some(false)
        );
        assert_eq!(
            report.instrument_admits(gate::GateId::TestSubsumption),
            Some(false)
        );
        let document = String::from_utf8(report.encode().unwrap()).unwrap();
        assert!(document.contains(COVERAGE_UNKNOWN));
        assert!(document.contains("mutant src/lib.rs:9 replace + with -"));
        assert!(document.contains("gate::lib"));
    }

    #[test]
    fn one_join_refuses_mismatched_selected_sources_before_reporting() {
        let tests = CanonicalTestExecution::new(
            source(b"workspace tests"),
            vec!["src/lib.rs".to_owned()],
            vec!["gate::lib".to_owned()],
            Vec::new(),
            CoverageCompleteness::unknown(COVERAGE_UNKNOWN).unwrap(),
        )
        .unwrap();
        joined_report(tests, mutation(source(b"mutation"))).unwrap_err();
    }

    #[test]
    fn selected_test_build_roots_replace_ambient_writable_roots() {
        let environment = Environment::new([
            (OsString::from("ZED"), OsString::from("last")),
            (
                OsString::from(CARGO_TARGET_DIR),
                OsString::from("/ambient/target"),
            ),
        ])
        .unwrap();
        let selected = selected_test_environment(environment, Path::new("/repo")).unwrap();
        let entries = selected
            .entries()
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            entries.get(OsStr::new(CARGO_TARGET_DIR)),
            Some(&OsString::from("/repo/target/test-evidence/cargo-target"))
        );
        assert_eq!(
            entries.get(OsStr::new(CARGO_BUILD_BUILD_DIR)),
            Some(&OsString::from("/repo/target/test-evidence/cargo-build"))
        );
    }
}
