//! Contract coverage for the shared long-running test-evidence authority.

use gate::{
    CanonicalTestExecution, CoverageCompleteness, CoveredSource, GateId, MutantDisposition,
    MutationExecution, MutationObservation, StandingMutationWaiver, SubsumptionRelation,
    SurvivorRate, TestCoverage, TestEvidenceGraph, TestEvidenceStanding,
};
use wire::{Digest, Domain};

fn source(label: &[u8]) -> Digest {
    Digest::compute(Domain::GateTree, label)
}

fn covered(test: &str, path: &str, lines: &[u32]) -> TestCoverage {
    TestCoverage::new(
        test,
        vec![CoveredSource::new(path, lines.iter().copied()).expect("covered source")],
    )
    .expect("test coverage")
}

fn mutation(
    source: &Digest,
    floor: Option<SurvivorRate>,
    dispositions: &[(&str, &str, MutantDisposition)],
) -> MutationExecution {
    MutationExecution::new(
        source.clone(),
        floor,
        dispositions
            .iter()
            .map(|(identity, path, disposition)| {
                MutationObservation::new(*identity, *path, *disposition).expect("mutation")
            })
            .collect(),
    )
    .expect("mutation execution")
}

fn graph(
    source: &Digest,
    changed: &[&str],
    tests: Vec<TestCoverage>,
    coverage: CoverageCompleteness,
    mutation: MutationExecution,
    waivers: Vec<StandingMutationWaiver>,
) -> TestEvidenceGraph {
    let tests_run = tests
        .iter()
        .map(|test| test.test().to_owned())
        .collect::<Vec<_>>();
    graph_with_run(
        source, changed, tests_run, tests, coverage, mutation, waivers,
    )
}

fn graph_with_run(
    source: &Digest,
    changed: &[&str],
    tests_run: Vec<String>,
    test_coverage: Vec<TestCoverage>,
    coverage: CoverageCompleteness,
    mutation: MutationExecution,
    waivers: Vec<StandingMutationWaiver>,
) -> TestEvidenceGraph {
    let tests = CanonicalTestExecution::new(
        source.clone(),
        changed.iter().map(|path| (*path).to_owned()).collect(),
        tests_run,
        test_coverage,
        coverage,
    )
    .expect("canonical test execution");
    TestEvidenceGraph::new(tests, mutation, waivers).expect("shared graph")
}

#[test]
fn one_graph_refuses_test_and_mutation_executions_from_different_sources() {
    let tests = CanonicalTestExecution::new(
        source(b"tests"),
        vec!["src/lib.rs".to_owned()],
        vec!["suite::one".to_owned()],
        vec![covered("suite::one", "src/lib.rs", &[1])],
        CoverageCompleteness::Complete,
    )
    .unwrap();
    let mutation = mutation(
        &source(b"mutation"),
        Some(SurvivorRate::new(0, 1).unwrap()),
        &[(
            "mutant-one",
            "src/lib.rs",
            MutantDisposition::ConfirmedKilled,
        )],
    );

    assert!(TestEvidenceGraph::new(tests, mutation, Vec::new()).is_err());
}

#[test]
fn suite_seam_is_binary_over_changed_sources_and_tests_that_actually_ran() {
    let source = source(b"suite-seam");
    let mutation = mutation(
        &source,
        Some(SurvivorRate::new(0, 1).unwrap()),
        &[(
            "mutant-one",
            "src/shared.rs",
            MutantDisposition::ConfirmedKilled,
        )],
    );
    let graph = graph_with_run(
        &source,
        &["src/shared.rs", "src/unreached.rs"],
        vec!["suite::shared".to_owned()],
        vec![
            covered("suite::shared", "src/shared.rs", &[4, 9]),
            covered("suite::unrun", "src/shared.rs", &[4]),
        ],
        CoverageCompleteness::Complete,
        mutation,
        Vec::new(),
    );

    let report = graph.suite_seam();
    assert_eq!(report.gate_id(), GateId::SuiteSeam);
    assert_eq!(report.standing(), TestEvidenceStanding::Findings);
    assert!(!report.admits());
    assert_eq!(report.findings().len(), 1);
    assert_eq!(report.findings()[0].test(), "suite::unrun");
    assert_eq!(report.findings()[0].source(), "src/shared.rs");
    assert_eq!(
        graph.report().instrument_admits(GateId::SuiteSeam),
        Some(false)
    );
}

#[test]
fn unknown_and_incomplete_coverage_are_loud_and_non_clean_in_every_fold() {
    for coverage in [
        CoverageCompleteness::unknown("coverage publisher was unavailable").unwrap(),
        CoverageCompleteness::incomplete("2 of 3 test contexts arrived").unwrap(),
    ] {
        let source = source(format!("{coverage:?}").as_bytes());
        let mutation = mutation(
            &source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            &[(
                "mutant-one",
                "src/lib.rs",
                MutantDisposition::ConfirmedKilled,
            )],
        );
        let graph = graph(
            &source,
            &["src/lib.rs"],
            vec![covered("suite::one", "src/lib.rs", &[1])],
            coverage,
            mutation,
            Vec::new(),
        );

        let suite = graph.suite_seam();
        let mutation = graph.mutation_coverage();
        let subsumption = graph.test_subsumption();
        assert_eq!(suite.standing(), TestEvidenceStanding::Unknown);
        assert_eq!(mutation.standing(), TestEvidenceStanding::Unknown);
        assert_eq!(subsumption.standing(), TestEvidenceStanding::Unknown);
        assert!(!suite.admits());
        assert!(!mutation.admits());
        assert!(!subsumption.admits());
        let combined = graph.report();
        assert_eq!(combined.instrument_admits(GateId::SuiteSeam), Some(false));
        assert_eq!(
            combined.instrument_admits(GateId::MutationCoverage),
            Some(false)
        );
        assert_eq!(
            combined.instrument_admits(GateId::TestSubsumption),
            Some(false)
        );
    }
}

#[test]
fn one_combined_report_retains_the_shared_evidence_and_every_fold() {
    let source = source(b"combined-report");
    let graph = graph(
        &source,
        &["src/lib.rs"],
        vec![covered("suite::one", "src/lib.rs", &[3, 8])],
        CoverageCompleteness::Complete,
        mutation(
            &source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            &[(
                "mutant src/lib.rs:8 replace true with false",
                "src/lib.rs",
                MutantDisposition::ConfirmedKilled,
            )],
        ),
        Vec::new(),
    );

    let report = graph.report();
    assert_eq!(report.version(), gate::TEST_EVIDENCE_REPORT_VERSION);
    assert_eq!(report.source(), &source);
    assert_eq!(report.tests().tests_run(), ["suite::one"]);
    assert_eq!(report.mutation().mutants()[0].source(), "src/lib.rs");
    assert_eq!(report.instrument_admits(GateId::SuiteSeam), Some(true));
    assert_eq!(
        report.instrument_admits(GateId::MutationCoverage),
        Some(true)
    );
    assert_eq!(
        report.instrument_admits(GateId::TestSubsumption),
        Some(true)
    );
    assert_eq!(report.instrument_admits(GateId::FileShape), None);

    let document = String::from_utf8(report.encode().unwrap()).unwrap();
    assert!(document.contains(source.as_str()));
    assert!(document.contains("suite::one"));
    assert!(document.contains("src/lib.rs"));
    assert!(document.contains("mutant src/lib.rs:8 replace true with false"));
    assert!(document.contains("\"suite_seam\""));
    assert!(document.contains("\"mutation_coverage\""));
    assert!(document.contains("\"test_subsumption\""));
}

#[test]
fn mutation_coverage_fails_only_on_unwaived_regression() {
    let source = source(b"mutation-regression");
    let floor = SurvivorRate::new(1, 2).unwrap();
    let dispositions = [
        (
            "survivor-a",
            "src/a.rs",
            MutantDisposition::ConfirmedSurvived,
        ),
        (
            "survivor-b",
            "src/b.rs",
            MutantDisposition::ConfirmedSurvived,
        ),
    ];
    let regressed = graph(
        &source,
        &["src/a.rs"],
        vec![covered("suite::one", "src/a.rs", &[1])],
        CoverageCompleteness::Complete,
        mutation(&source, Some(floor), &dispositions),
        Vec::new(),
    )
    .mutation_coverage();
    assert_eq!(regressed.gate_id(), GateId::MutationCoverage);
    assert_eq!(regressed.standing(), TestEvidenceStanding::Findings);
    assert!(!regressed.admits());
    assert_eq!(regressed.findings().len(), 2);
    assert_eq!(regressed.findings()[0].source(), "src/a.rs");

    let waiver = StandingMutationWaiver::new(
        "survivor-b",
        "src/b.rs",
        "platform adapter intentionally preserves both branches",
    )
    .unwrap();
    let held = graph(
        &source,
        &["src/a.rs"],
        vec![covered("suite::one", "src/a.rs", &[1])],
        CoverageCompleteness::Complete,
        mutation(&source, Some(floor), &dispositions),
        vec![waiver],
    )
    .mutation_coverage();
    assert_eq!(held.standing(), TestEvidenceStanding::Clean);
    assert!(held.admits());
    assert!(held.findings().is_empty());
    assert_eq!(held.measured(), Some(floor));
}

#[test]
fn missing_floor_and_unconfirmed_mutants_are_unknown_not_clean_or_regressed() {
    let source = source(b"mutation-unknown");
    for mutation in [
        mutation(
            &source,
            None,
            &[(
                "survivor",
                "src/lib.rs",
                MutantDisposition::ConfirmedSurvived,
            )],
        ),
        mutation(
            &source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            &[("flaky", "src/lib.rs", MutantDisposition::Unconfirmed)],
        ),
    ] {
        let report = graph(
            &source,
            &["src/lib.rs"],
            vec![covered("suite::one", "src/lib.rs", &[1])],
            CoverageCompleteness::Complete,
            mutation,
            Vec::new(),
        )
        .mutation_coverage();
        assert_eq!(report.standing(), TestEvidenceStanding::Unknown);
        assert!(!report.admits());
        assert!(report.reason().is_some());
    }
}

#[test]
fn stale_reasoned_waivers_are_advisory_and_cannot_create_a_regression() {
    let source = source(b"stale-waiver");
    let waiver = StandingMutationWaiver::new(
        "removed-survivor",
        "src/lib.rs",
        "standing rationale remains reviewable",
    )
    .unwrap();
    let report = graph(
        &source,
        &["src/lib.rs"],
        vec![covered("suite::one", "src/lib.rs", &[1])],
        CoverageCompleteness::Complete,
        mutation(
            &source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            &[("killed", "src/lib.rs", MutantDisposition::ConfirmedKilled)],
        ),
        vec![waiver],
    )
    .mutation_coverage();

    assert_eq!(report.standing(), TestEvidenceStanding::Clean);
    assert!(report.admits());
    assert_eq!(report.stale_waivers().len(), 1);
    assert_eq!(
        report.stale_waivers()[0].reason(),
        "standing rationale remains reviewable"
    );
}

#[test]
fn subsumption_is_advisory_deterministic_and_retains_exact_source_provenance() {
    let source = source(b"subsumption");
    let graph = graph(
        &source,
        &["src/lib.rs"],
        vec![
            covered("suite::narrow", "src/lib.rs", &[1]),
            covered("suite::wide-a", "src/lib.rs", &[1, 2]),
            covered("suite::wide-b", "src/lib.rs", &[1, 2]),
        ],
        CoverageCompleteness::Complete,
        mutation(
            &source,
            Some(SurvivorRate::new(0, 1).unwrap()),
            &[("killed", "src/lib.rs", MutantDisposition::ConfirmedKilled)],
        ),
        Vec::new(),
    );

    let report = graph.test_subsumption();
    assert_eq!(report.gate_id(), GateId::TestSubsumption);
    assert_eq!(report.standing(), TestEvidenceStanding::Findings);
    assert!(report.admits(), "subsumption findings are non-blocking");
    assert_eq!(report.findings().len(), 2);

    let narrow = report
        .findings()
        .iter()
        .find(|finding| finding.test() == "suite::narrow")
        .unwrap();
    assert_eq!(narrow.contained_by(), "suite::wide-a");
    assert_eq!(narrow.relation(), SubsumptionRelation::StrictSubset);
    assert_eq!(narrow.covered_sources()[0].path(), "src/lib.rs");
    assert_eq!(
        narrow.covered_sources()[0]
            .lines()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![1]
    );

    let equal = report
        .findings()
        .iter()
        .find(|finding| finding.test() == "suite::wide-b")
        .unwrap();
    assert_eq!(equal.contained_by(), "suite::wide-a");
    assert_eq!(equal.relation(), SubsumptionRelation::Equal);
    assert_eq!(
        graph.report().instrument_admits(GateId::TestSubsumption),
        Some(true),
        "measured subsumption findings remain advisory"
    );
}
