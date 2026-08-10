use super::*;
use crate::metrics::{EvidenceState, MetricId, MetricKey, StructuralArmEvidence};

/// Each result carries evidence its own family owns, refusals included.
pub(super) fn assert_every_family_projected_its_own_evidence(report: &BatteryReport) {
    assert_eq!(report.results().len(), GateId::BATTERY.len());
    for result in report.results() {
        match result.family() {
            ArmFamily::Core => {
                assert!(matches!(result.evidence(), ArmEvidence::Core { .. }));
            }
            ArmFamily::Toolchain => {
                assert!(matches!(result.evidence(), ArmEvidence::Toolchain { .. }));
            }
            ArmFamily::Metrics => assert_metric_arm_acquired(result),
            ArmFamily::Hygiene => {
                assert!(matches!(result.evidence(), ArmEvidence::Hygiene { .. }));
            }
        }
    }
    assert!(
        report.results().iter().all(|result| {
            !matches!(
                result.evidence(),
                ArmEvidence::Hygiene { report }
                    if report.outcome == crate::HygieneOutcome::ParseFailure
            )
        }),
        "an analyzer read the broken file the sealed tree never held"
    );
}

/// One metric arm carried its own acquired evidence rather than a family refusal.
fn assert_metric_arm_acquired(result: &BatteryArmResult) {
    let ArmEvidence::Metrics { report } = result.evidence() else {
        panic!(
            "the metrics family projects its own acquired evidence, but {} carried {:?}",
            result.gate_id(),
            result.evidence()
        )
    };
    assert_eq!(report.gate_id(), result.gate_id());
    let (_, expected, evidence) = ACQUIRED_METRIC_ARMS
        .iter()
        .find(|(gate_id, ..)| *gate_id == result.gate_id())
        .unwrap_or_else(|| {
            panic!(
                "{} is claimed by no metric-arm row, so nothing states what it should report: {}",
                result.gate_id(),
                result.reason()
            )
        });
    assert_eq!(
        result.state(),
        *expected,
        "{} reported the wrong state for this fixture: {}",
        result.gate_id(),
        result.reason()
    );
    assert!(
        result.reason().contains(evidence),
        "{} names the evidence its state rests on, but {evidence:?} is absent from: {}",
        result.gate_id(),
        result.reason()
    );
    if result.gate_id() == GateId::Complexity {
        assert_complexity_measured_the_fixture_routine(report);
    }
}

/// The complexity arm's pass rests on rows it measured, not on an empty scope.
fn assert_complexity_measured_the_fixture_routine(report: &StructuralArmEvidence) {
    let measured: Vec<_> = report
        .metrics()
        .iter()
        .filter_map(|evidence| match evidence.state() {
            EvidenceState::Measured { measurement } => Some((
                evidence.metric(),
                evidence.key().clone(),
                measurement.value(),
            )),
            _ => None,
        })
        .collect();
    let key = |path: &str, function: &str| {
        MetricKey::function(path, function).expect("a fixture routine keys")
    };
    assert_eq!(
        measured,
        [
            (MetricId::RoutineCcn, key("app/main.py", "entry"), 1),
            (MetricId::RoutineCcn, key("src/lib.rs", "fixture"), 1),
            (
                MetricId::RoutineCcn,
                key("tests/test_main.py", "test_entry"),
                1
            ),
            (MetricId::RoutineLength, key("app/main.py", "entry"), 2),
            (MetricId::RoutineLength, key("src/lib.rs", "fixture"), 1),
            (
                MetricId::RoutineLength,
                key("tests/test_main.py", "test_entry"),
                2
            ),
        ],
        "the complexity arm passed on rows it measured from the fixture's own sources"
    );
}

/// The two Git-declared arms answer for the repository as it stood at capture.
pub(super) fn assert_the_git_arms_kept_the_capture(report: &BatteryReport) {
    let observation = |gate_id: GateId| {
        let result = report
            .results()
            .iter()
            .find(|result| result.gate_id() == gate_id)
            .expect("the battery ran every compiled arm");
        let ArmEvidence::Core { report: arm } = result.evidence() else {
            panic!("{} is a core arm", gate_id.as_str())
        };
        arm.observation().clone()
    };
    assert_eq!(
        observation(GateId::Staging),
        CoreObservation::Measured {
            evidence: CoreEvidence::Staging(
                StagingEvidence::new(Vec::<String>::new()).expect("clean staging evidence")
            )
        },
        "staging read a partial staging created after the capture it is labelled with"
    );
    assert_eq!(
        observation(GateId::MergeIntegrity),
        CoreObservation::Measured {
            evidence: CoreEvidence::MergeIntegrity(MergeIntegrityEvidence::NoMerge)
        },
        "merge integrity read a merge started after the capture it is labelled with"
    );
}

/// The scanning arms read the change inventory the executor captured.
#[test]
fn the_scanning_arms_report_exact_findings_over_the_captured_changes() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let (word, replacement) = discouraged_word();
    let marked = format!("// {LOCAL_PATH_MARKER}somebody/devel\n");
    let prose = format!("This paragraph is {word} and nothing else.\n");
    let column = u32::try_from(prose.find(&word).expect("the fixture names the word") + 1)
        .expect("a fixture column fits");
    let marker_column = u32::try_from(
        marked
            .find(LOCAL_PATH_MARKER)
            .expect("the fixture names the marker")
            + 1,
    )
    .expect("a fixture column fits");
    fs::write(repository.join(MARKED), &marked).expect("fixture marked source");
    fs::create_dir(repository.join("docs")).expect("fixture prose directory");
    fs::write(repository.join(PROSE), &prose).expect("fixture discouraged prose");
    fs::write(
        repository.join("docs/unselected.md"),
        format!("{marked}This paragraph is {word} as well.\n"),
    )
    .expect("fixture change nobody selected");
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "scanned sources"]);

    let hygiene = crate::HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("production hygiene roots");
    let mut capture = crate::BatteryCapture::capture(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting("scanning-actor"),
        crate::RimCapture::new(
            &repository,
            None,
            [MARKED, PROSE].map(str::to_owned).to_vec(),
        )
        .expect("the fixture repository root is absolute"),
        hygiene,
    )
    .expect("capture production battery source");
    let report = capture.execute().expect("complete production battery");

    let scanned = |gate_id: GateId| {
        let result = report
            .results()
            .iter()
            .find(|result| result.gate_id() == gate_id)
            .expect("the battery ran every compiled arm");
        let ArmEvidence::Toolchain { report } = result.evidence() else {
            panic!("{} is a toolchain arm", gate_id.as_str())
        };
        assert_eq!(
            report.outcome,
            ArmOutcome::Findings,
            "{} admitted a selected change it should have reported: {}",
            gate_id.as_str(),
            report.reason
        );
        assert_eq!(result.state(), BatteryState::Failed);
        report.findings.clone()
    };

    assert_eq!(
        scanned(GateId::LocalPaths),
        [ArmFinding {
            path: Some(MARKED.to_owned()),
            line: Some(1),
            column: Some(marker_column),
            detail: FindingDetail::LocalPath,
        }],
        "the marker scanner named something other than the one selected marked line"
    );
    assert_eq!(
        scanned(GateId::Taste),
        [ArmFinding {
            path: Some(PROSE.to_owned()),
            line: Some(1),
            column: Some(column),
            detail: FindingDetail::Taste { word, replacement },
        }],
        "the prose scanner named something other than the one selected discouraged word"
    );
}

#[test]
fn every_nonpass_state_refuses_without_erasing_later_results() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::nonpassing(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("total nonpass report");

    assert_eq!(executor.calls.len(), 27);
    assert_eq!(executor.calls.last(), Some(&GateId::PrivateInternals));
    assert_eq!(report.results().len(), 27);
    assert!(!report.admits());
    for state in [
        BatteryState::Failed,
        BatteryState::Unavailable,
        BatteryState::Unsupported,
        BatteryState::Unmeasured,
        BatteryState::TimedOut,
        BatteryState::ExplicitSkip,
        BatteryState::NotApplicable,
        BatteryState::Excluded,
    ] {
        assert!(
            report
                .results()
                .iter()
                .any(|result| result.state() == state)
        );
    }
    for result in report.results().iter().filter(|result| {
        matches!(
            result.state(),
            BatteryState::NotApplicable | BatteryState::Excluded
        )
    }) {
        let provenance = result.provenance().expect("admitted skip provenance");
        let expected = result
            .evidence()
            .provenance()
            .expect("typed provenance")
            .expect("skip provenance");
        assert_eq!(provenance, &expected);
    }
}

#[test]
fn executor_refusals_and_invalid_evidence_stay_visible_and_total() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::faulting(&fixture.source, 3, 4);
    let report = run_battery(&inputs, &mut executor).expect("total executor-failure report");

    assert_eq!(executor.calls, GateId::BATTERY);
    assert_eq!(report.results().len(), 27);
    assert_eq!(report.results()[3].state(), BatteryState::Failed);
    assert_eq!(
        report.results()[3].reason(),
        "executor refused arm: provider\\nlost"
    );
    assert_eq!(report.results()[4].state(), BatteryState::Failed);
    assert!(
        report.results()[4]
            .reason()
            .starts_with("executor returned invalid evidence:")
    );
    assert_eq!(
        report.results().last().expect("last result").gate_id(),
        GateId::PrivateInternals
    );

    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");
    assert_refusals(vec![(
        "executor failure names a different compiled declaration",
        mutate(&value, |candidate| {
            candidate["results"][3]["evidence"]["declaration"] =
                serde_json::json!("gate::core::staging::forged-v1")
        }),
    )]);
}
