//! Running the compiled battery: one capture, one pass, every arm once.

use super::*;

#[test]
fn runner_executes_all_25_once_in_order_with_complete_context() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete battery");

    assert_eq!(executor.calls, GateId::ALL);
    assert_eq!(report.results().len(), GateId::ALL.len());
    assert!(report.admits());
    assert_eq!(report.context().source(), &fixture.source);
    assert_eq!(report.context().actor().source(), "active-actor");
    assert_eq!(report.context().actor().value(), "fixture-actor");
    assert_eq!(report.context().scope().name(), "selected-index");
    assert_eq!(
        report.context().scope().provenance(),
        "compiled test selector"
    );
    assert_eq!(report.context().configuration().len(), 1);
    assert_eq!(
        report.context().configuration()[0].key(),
        "policy.test_paths"
    );
    assert_eq!(report.context().configuration()[0].release(), "current");
    assert_eq!(
        report.context().configuration()[0].layer(),
        OriginLayer::Repository
    );
    assert_eq!(report.context().configuration()[0].document(), "spice.toml");
    assert_eq!(
        report.context().configuration()[0].field(),
        &["policy".to_owned(), "test_paths".to_owned()]
    );
    assert_eq!(report.context().bounds().len(), 1);
    assert_eq!(report.context().bounds()[0].rule(), "file-shape");
    assert_eq!(report.context().bounds()[0].measured(), 101);
    assert_eq!(report.context().bounds()[0].active_limit(), 152);
    assert_eq!(report.context().bounds()[0].latch_transition(), "clear");
    assert_eq!(
        report.context().bounds()[0]
            .actor()
            .expect("bound actor")
            .value(),
        "fixture-actor"
    );
    assert_eq!(
        report.context().bounds()[0].actor_inputs().candidate_path(),
        "src/lib.rs"
    );
    assert_eq!(
        report.context().bounds()[0].actor_inputs().active_actor(),
        Some("fixture-actor")
    );
    assert_eq!(report.context().latches().len(), 1);
    assert_eq!(report.context().latches()[0].kind(), "file-loc");
    assert_eq!(report.context().latches()[0].path(), "src/lib.rs");
    assert_eq!(report.context().latches()[0].function(), None);
    assert_eq!(report.context().tools().len(), 1);
    assert_eq!(report.context().tools()[0].name(), "cargo");
    assert_eq!(report.context().tools()[0].program(), "cargo");
    assert!(
        report
            .context()
            .tools()
            .iter()
            .all(|tool| { !matches!(tool.program(), "python" | "task" | "sh" | "bash" | "zsh") })
    );
    assert_eq!(report.context().tools()[0].version(), Some("cargo fixture"));
    assert_eq!(
        report.context().tools()[0].executable_digest(),
        Some(&Digest::compute(Domain::GateTool, b"fixture cargo"))
    );
    assert_eq!(report.context().policy(), &fixture.policy_digest());
    assert_eq!(report.context().state(), &fixture.state.before().digest());

    for (position, (result, expected)) in report.results().iter().zip(GateId::ALL).enumerate() {
        assert_eq!(result.position(), position);
        assert_eq!(result.predecessor(), expected.predecessor_name());
        assert_eq!(result.gate_id(), expected);
        assert_eq!(result.state(), BatteryState::Passed);
        assert_eq!(result.evidence().gate_id(), expected);
        assert_eq!(
            result.evidence().state().expect("typed state"),
            BatteryState::Passed
        );
        assert!(!result.evidence().reason().expect("typed reason").is_empty());
        assert_family_evidence(result.evidence(), &fixture.source);
    }

    let encoded = report.encode().expect("canonical report");
    assert_eq!(BatteryReport::decode(&encoded).expect("decode"), report);
}

/// One capture answers for the whole battery, including its Git-derived arms.
///
/// The repository is moved after capture in every direction a core arm reads:
/// an unstaged edit, a file left partially staged, and a merge in progress.
/// Each mutation is proved real with the same program the arms run, so the
/// clean observations below are a binding rather than a scanner that never saw
/// anything. An executor that acquired the core family on first request would
/// report the partial staging and the merge, and its report would name a
/// capture that never held either.
#[test]
fn repository_executor_owns_one_capture_and_runs_each_family_once() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let hygiene = crate::HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("production hygiene roots");
    let selector = crate::Selector::staged();
    let mut events = Vec::new();
    let mut record = |event| events.push(event);
    let changes = ["app/main.py", "src/lib.rs", "tests/test_main.py"]
        .map(str::to_owned)
        .to_vec();
    let mut executor = RepositoryExecutor::capture_traced(
        &repository,
        &selector,
        crate::ClaimCapture::Idle,
        crate::ActorCapture::Absent,
        crate::RimCapture::new(&repository, None, changes),
        hygiene,
        &mut record,
    )
    .expect("capture production battery source");
    let source = executor.source().clone();
    assert_ne!(executor.root(), repository);

    move_the_repository_after_capture(&repository);

    let inputs = fixture.inputs_for(&source);
    let report = run_battery(&inputs, &mut executor).expect("complete production battery");
    drop(executor);

    assert_each_family_ran_once(&events);
    assert_every_family_projected_its_own_evidence(&report);
    assert_the_git_arms_kept_the_capture(&report);

    let encoded = report.encode().expect("encode production report");
    assert_eq!(
        BatteryReport::decode(&encoded).expect("decode production report"),
        report
    );
    let receipt = BatteryReceipt::mint(report, &fixture_baseline("production executor"));
    let receipt_bytes = receipt.encode().expect("encode production receipt");
    assert_eq!(
        BatteryReceipt::verify(&receipt_bytes).expect("verify production receipt"),
        receipt
    );
}

/// Move the captured repository in every direction a core arm can observe.
///
/// Each mutation is then proved real with the same program the Git-declared
/// arms run, so a caller asserting that the battery ignored all of it is
/// asserting against a repository that genuinely changed underneath it.
fn move_the_repository_after_capture(repository: &Path) {
    fs::write(repository.join("tests/test_main.py"), "def test_broken(:\n")
        .expect("edit an unstaged file after capture");
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 2\n",
    )
    .expect("stage new content after capture");
    fixture_git(repository, &["add", "app/main.py"]);
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 3\n",
    )
    .expect("leave that staged file dirty after capture");
    let head = fixture_git(repository, &["rev-parse", "HEAD"]);
    fs::write(repository.join(".git/MERGE_HEAD"), format!("{head}\n"))
        .expect("start a merge after capture");

    assert_eq!(
        fixture_git(repository, &["diff", "--cached", "--name-only"]),
        "app/main.py",
        "the mutation must really stage the file the staging arm reads"
    );
    assert!(
        fixture_git(repository, &["diff", "--name-only"])
            .lines()
            .any(|path| path == "app/main.py"),
        "the mutation must really leave that same file unstaged as well"
    );
}

/// Every canonical arm is invoked once and no family is acquired twice.
fn assert_each_family_ran_once(events: &[RepositoryExecutionEvent]) {
    let arm_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::ArmInvoked { gate_id, .. } => Some(*gate_id),
            RepositoryExecutionEvent::FamilyInvoked { .. } => None,
        })
        .collect();
    assert_eq!(arm_events, GateId::ALL);

    let family_events: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::FamilyInvoked { family } => Some(*family),
            RepositoryExecutionEvent::ArmInvoked { .. } => None,
        })
        .collect();
    assert_eq!(
        family_events,
        [
            ArmFamily::Core,
            ArmFamily::Toolchain,
            ArmFamily::Metrics,
            ArmFamily::Hygiene,
        ]
    );
}

/// Each result carries evidence its own family owns, refusals included.
fn assert_every_family_projected_its_own_evidence(report: &BatteryReport) {
    assert_eq!(report.results().len(), GateId::ALL.len());
    for result in report.results() {
        match result.family() {
            ArmFamily::Core => {
                assert!(matches!(result.evidence(), ArmEvidence::Core { .. }));
            }
            ArmFamily::Toolchain => {
                assert!(matches!(result.evidence(), ArmEvidence::Toolchain { .. }));
            }
            ArmFamily::Metrics => {
                let ArmEvidence::ExecutorFailure {
                    gate_id,
                    arm_family,
                    declaration,
                    reason,
                } = result.evidence()
                else {
                    panic!("unavailable metrics acquisition must fail explicitly")
                };
                assert_eq!(*gate_id, result.gate_id());
                assert_eq!(*arm_family, ArmFamily::Metrics);
                assert_eq!(declaration, result.declaration());
                assert!(reason.contains("metrics family acquisition is unavailable"));
            }
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

/// The two Git-declared arms answer for the repository as it stood at capture.
fn assert_the_git_arms_kept_the_capture(report: &BatteryReport) {
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
///
/// `LocalPaths` and `Taste` scan the selected changes and nothing else, so the
/// list handed in at capture is the whole of what they open. The unselected
/// file is the control: it is committed, dirty for both arms, and named by
/// nobody. An executor that recomputed the change set from the repository would
/// report it, and one that substituted an empty list would report neither file
/// and call a marked tree clean.
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
    let mut executor = RepositoryExecutor::capture(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        crate::ActorCapture::Absent,
        crate::RimCapture::new(
            &repository,
            None,
            [MARKED, PROSE].map(str::to_owned).to_vec(),
        ),
        hygiene,
    )
    .expect("capture production battery source");
    let source = executor.source().clone();
    let inputs = fixture.inputs_for(&source);
    let report = run_battery(&inputs, &mut executor).expect("complete production battery");

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

    assert_eq!(executor.calls.len(), 25);
    assert_eq!(executor.calls.last(), Some(&GateId::PrivateInternals));
    assert_eq!(report.results().len(), 25);
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

    assert_eq!(executor.calls, GateId::ALL);
    assert_eq!(report.results().len(), 25);
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
