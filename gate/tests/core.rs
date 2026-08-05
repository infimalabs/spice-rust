use std::fs;
use std::str::FromStr;

use gate::{
    ActorCapture, CORE_GATE_IDS, ClaimCapture, ConfigInventoryEvidence, ConfigKeyEvidence,
    Contract, CoreArmInput, CoreEvidence, CoreFinding, CoreInputs, CoreObservation, CoreProvenance,
    CoreReport, CoreReportDecodeError, CoreSource, CoreStanding, CoreTimeoutContract,
    CoreToolContract, DocMeasurement, GateId, MergeIntegrityEvidence, MergeTreeCompletion,
    PREDECESSOR_GATE_RELEASE, PlanPhaseEvidence, RepoDocLimit, RepoDocsEvidence, RepoShapeEvidence,
    Selector, StagingEvidence, acquire_core, core_declarations, evaluate_core,
};
use serde::Deserialize;
use serde_json::Value;
use wire::{Digest, Domain, Phase, TaskHandle};

mod support;

use support::{
    acquired, acquired_arm, contract_repository, git, git_commit, git_output, git_repository,
    measured,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParityFixture {
    release: String,
    cases: Vec<ParityCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParityCase {
    gate: GateId,
    kind: CaseKind,
    source: String,
    observation: String,
    expected: CoreStanding,
    diagnostic: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CaseKind {
    Pass,
    Fail,
    StagedPass,
    SameTreePass,
    ConflictedFail,
    UnnamedConflictFail,
    PeerFail,
    UnlimitedPass,
    Unavailable,
    Unsupported,
    Unmeasured,
    TimedOut,
    Skipped,
    ShapeSkipped,
}

#[test]
fn declarations_fix_identity_order_sources_schemas_and_tool_contracts() {
    let declarations = core_declarations();
    let ids: Vec<_> = declarations
        .iter()
        .map(|declaration| declaration.id())
        .collect();
    assert_eq!(ids, CORE_GATE_IDS);
    assert_eq!(ids, GateId::ALL[..CORE_GATE_IDS.len()]);
    assert_eq!(
        declarations
            .iter()
            .map(|declaration| declaration.schema())
            .collect::<Vec<_>>(),
        [
            "merge-tree-comparison-v1",
            "active-claim-phase-v1",
            "repo-shape-findings-v1",
            "partial-staging-paths-v1",
            "repo-doc-character-bounds-v1",
            "config-admission-findings-v1",
            "config-declaration-consumer-inventory-v1",
            "config-declaration-consumer-inventory-v1",
        ]
    );

    for declaration in declarations {
        assert!(!declaration.schema().is_empty());
        let predecessor = declaration.predecessor();
        assert_eq!(predecessor.release(), PREDECESSOR_GATE_RELEASE);
        assert!(!predecessor.source().is_empty());
        assert!(!predecessor.observation().is_empty());
        match declaration.tool() {
            CoreToolContract::Compiled { provider } => assert!(!provider.is_empty()),
            CoreToolContract::Git { commands, timeout } => {
                assert_eq!(timeout, CoreTimeoutContract::Required);
                assert!(!commands.is_empty());
                for command in commands {
                    assert!(!command.arguments().is_empty());
                    assert!(!command.accepted_exit_codes().is_empty());
                }
            }
        }
    }

    let CoreToolContract::Git {
        commands: merge,
        timeout: _,
    } = declaration(GateId::MergeIntegrity).tool()
    else {
        panic!("merge integrity must use bounded git acquisition");
    };
    assert_eq!(merge.len(), 5);
    assert_eq!(
        merge[3].arguments(),
        [
            "merge-tree",
            "--write-tree",
            "-z",
            "--name-only",
            "HEAD",
            "MERGE_HEAD",
        ]
    );
    assert_eq!(merge[3].accepted_exit_codes(), [0, 1]);

    let CoreToolContract::Git {
        commands: staging,
        timeout: _,
    } = declaration(GateId::Staging).tool()
    else {
        panic!("staging must use bounded git acquisition");
    };
    assert_eq!(staging.len(), 2);
    assert_eq!(
        staging[0].arguments(),
        ["diff", "--cached", "--name-only", "--diff-filter=ACMR"]
    );
    assert_eq!(staging[0].accepted_exit_codes(), [0]);
}

#[test]
fn predecessor_fixture_covers_every_arm_and_every_typed_outcome() {
    let fixture: ParityFixture =
        toml::from_str(include_str!("fixtures/core/predecessor-parity.toml"))
            .expect("decode predecessor parity fixture");
    assert_eq!(fixture.release, PREDECESSOR_GATE_RELEASE);

    for id in CORE_GATE_IDS {
        assert!(
            fixture
                .cases
                .iter()
                .any(|case| { case.gate == id && matches!(case.kind, CaseKind::Pass) })
        );
        assert!(
            fixture
                .cases
                .iter()
                .any(|case| { case.gate == id && matches!(case.kind, CaseKind::Fail) })
        );
    }
    for standing in [
        CoreStanding::Unavailable,
        CoreStanding::Unsupported,
        CoreStanding::Unmeasured,
        CoreStanding::TimedOut,
        CoreStanding::Skipped,
    ] {
        assert!(fixture.cases.iter().any(|case| case.expected == standing));
    }

    for case in fixture.cases {
        assert_case(&fixture.release, &case);
    }
}

#[test]
fn immutable_input_admission_refuses_order_tree_and_evidence_drift() {
    let tree = fixture_tree();
    let mut arms = passing_arms(&tree);
    arms.swap(0, 1);
    assert!(CoreInputs::new(arms).is_err());

    let mut arms = passing_arms(&tree);
    arms[3] = arm(
        GateId::Staging,
        Digest::compute(Domain::GateTree, b"another tree"),
        measured(passing_evidence(GateId::Staging)),
    );
    assert!(CoreInputs::new(arms).is_err());

    let mismatch = CoreArmInput::new(
        GateId::PlanPhase,
        tree,
        provenance(GateId::PlanPhase),
        measured(passing_evidence(GateId::RepoShape)),
    );
    assert!(mismatch.is_err());
}

#[test]
fn incomplete_observation_admission_requires_explicit_detail_and_bounds() {
    let tree = fixture_tree();
    for observation in [
        CoreObservation::Unavailable {
            reason: String::new(),
        },
        CoreObservation::Unmeasured {
            reason: String::new(),
        },
        CoreObservation::Skipped {
            reason: String::new(),
        },
        CoreObservation::TimedOut { milliseconds: 0 },
    ] {
        assert!(
            CoreArmInput::new(
                GateId::MergeIntegrity,
                tree.clone(),
                provenance(GateId::MergeIntegrity),
                observation,
            )
            .is_err()
        );
    }
    assert!(
        CoreArmInput::new(
            GateId::MergeIntegrity,
            tree,
            provenance(GateId::MergeIntegrity),
            CoreObservation::Unsupported {
                reason: "legacy grammar".to_owned(),
                recovery: String::new(),
            },
        )
        .is_err()
    );
    assert!(
        CoreArmInput::new(
            GateId::Staging,
            fixture_tree(),
            provenance(GateId::Staging),
            CoreObservation::Skipped {
                reason: "operator requested bypass".to_owned(),
            },
        )
        .is_err()
    );
}

#[test]
fn merge_evidence_refuses_identical_staged_and_head_trees() {
    assert_merge_evidence_refused(staged_merge("1", "1"));
}

#[test]
fn merge_evidence_refuses_conflicts_on_a_clean_completion() {
    assert_merge_evidence_refused(compared_merge(
        "1",
        "2",
        MergeTreeCompletion::Clean,
        vec!["spice/src/lib.rs".to_owned()],
    ));
}

#[test]
fn real_modify_delete_conflict_with_the_head_tree_fails_with_path_provenance() {
    let repository = git_repository("core-merge-conflict");
    repository.write("f", "base\n");
    git_commit(&repository, "base");

    git(&repository, &["checkout", "-q", "-b", "side"]);
    fs::remove_file(repository.path("f")).expect("delete f on side");
    git_commit(&repository, "delete f");

    git(&repository, &["checkout", "-q", "main"]);
    repository.write("f", "modified on main\n");
    git_commit(&repository, "modify f");

    let head_tree = git(&repository, &["rev-parse", "HEAD^{tree}"]);
    let merge = git_output(
        &repository,
        &[
            "merge-tree",
            "--write-tree",
            "-z",
            "--name-only",
            "HEAD",
            "side",
        ],
    );
    assert_eq!(merge.status.code(), Some(1), "modify/delete must conflict");
    let fields = merge.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let expected_tree = String::from_utf8(fields[0].to_owned()).expect("tree is UTF-8");
    assert_eq!(expected_tree, head_tree, "conflicted tree reproduces HEAD");
    assert!(
        fields.iter().any(|field| *field == b"f"),
        "git merge-tree names the conflicted path: {:?}",
        String::from_utf8_lossy(&merge.stdout)
    );

    let tree = fixture_tree();
    let observation = measured(CoreEvidence::MergeIntegrity(
        MergeIntegrityEvidence::Compared {
            head_tree,
            expected_tree,
            completion: MergeTreeCompletion::Conflicted,
            conflicted_paths: vec!["f".to_owned()],
        },
    ));
    let report = evaluate_core(
        &CoreInputs::new(battery_for(&tree, GateId::MergeIntegrity, observation))
            .expect("real conflicted evidence admits for evaluation"),
    );
    let merge_arm = &report.arms()[0];
    assert_eq!(merge_arm.standing(), CoreStanding::Failed);
    assert!(merge_arm.diagnostic().contains("conflicted at: f"));

    repository.remove();
}

#[test]
fn repo_doc_evidence_refuses_duplicates_and_preserves_unlimited_bounds() {
    assert!(
        RepoDocsEvidence::new(vec![
            DocMeasurement::new("AGENTS.md", 10, 20, false).expect("doc measurement"),
            DocMeasurement::new("AGENTS.md", 11, 20, false).expect("doc measurement"),
        ])
        .is_err()
    );
    let unlimited = DocMeasurement::unlimited("AGENTS.md", 50).expect("unlimited document");
    assert_eq!(unlimited.bound(), RepoDocLimit::Unlimited);
    assert_eq!(unlimited.limit(), None);
}

fn assert_merge_evidence_refused(evidence: MergeIntegrityEvidence) {
    let observation = measured(CoreEvidence::MergeIntegrity(evidence));
    assert!(
        CoreArmInput::new(
            GateId::MergeIntegrity,
            fixture_tree(),
            provenance(GateId::MergeIntegrity),
            observation,
        )
        .is_err()
    );
}

#[test]
fn canonical_report_decode_refuses_unknown_reordered_and_forged_outcomes() {
    let report = evaluate_core(
        &CoreInputs::new(passing_arms(&fixture_tree())).expect("admit passing battery"),
    );
    let encoded = report.encode().expect("encode core report");
    assert_eq!(CoreReport::decode(&encoded).expect("decode report"), report);

    let mut unknown: Value = serde_json::from_slice(&encoded).expect("decode value");
    unknown["unknown"] = Value::Bool(true);
    assert!(matches!(
        CoreReport::decode(&wire::encode(&unknown).expect("encode unknown")),
        Err(CoreReportDecodeError::Json(_))
    ));

    let mut reordered: Value = serde_json::from_slice(&encoded).expect("decode value");
    reordered["arms"]
        .as_array_mut()
        .expect("arms array")
        .swap(0, 1);
    assert!(matches!(
        CoreReport::decode(&wire::encode(&reordered).expect("encode reordered")),
        Err(CoreReportDecodeError::Inconsistent(_))
    ));

    let mut forged: Value = serde_json::from_slice(&encoded).expect("decode value");
    forged["arms"][0]["standing"] = Value::String("failed".to_owned());
    assert!(matches!(
        CoreReport::decode(&wire::encode(&forged).expect("encode forged")),
        Err(CoreReportDecodeError::Inconsistent(_))
    ));

    let mut alternate = encoded;
    alternate.push(b'\n');
    let error = CoreReport::decode(&alternate).expect_err("alternate spelling refuses");
    assert!(error.canonical().is_some());
}

fn assert_case(release: &str, case: &ParityCase) {
    let declaration = declaration(case.gate);
    assert_eq!(declaration.predecessor().release(), release);
    assert_eq!(declaration.predecessor().source(), case.source);
    assert_eq!(declaration.predecessor().observation(), case.observation);

    let tree = fixture_tree();
    let report = evaluate_core(
        &CoreInputs::new(battery_for(&tree, case.gate, case_observation(case)))
            .expect("admit parity case"),
    );
    assert_eq!(
        report.arms().iter().map(|arm| arm.id()).collect::<Vec<_>>(),
        CORE_GATE_IDS
    );
    let arm = &report.arms()[CORE_GATE_IDS
        .iter()
        .position(|id| *id == case.gate)
        .expect("fixture gate is compiled")];
    assert_eq!(arm.standing(), case.expected);
    assert!(arm.diagnostic().contains(&case.diagnostic));
    assert_eq!(arm.tree(), &tree);
    assert_eq!(arm.provenance().release(), release);
    assert_eq!(arm.provenance().document(), case.source);
    assert_eq!(arm.provenance().observation(), case.observation);
    assert_eq!(
        arm.evidence().is_some(),
        matches!(
            case.kind,
            CaseKind::Pass
                | CaseKind::Fail
                | CaseKind::StagedPass
                | CaseKind::SameTreePass
                | CaseKind::ConflictedFail
                | CaseKind::UnnamedConflictFail
                | CaseKind::PeerFail
                | CaseKind::UnlimitedPass
        )
    );
    assert_eq!(report.admits(), case.expected.admits());

    let encoded = report.encode().expect("encode parity report");
    assert_eq!(
        CoreReport::decode(&encoded).expect("decode parity report"),
        report
    );
}

fn case_observation(case: &ParityCase) -> CoreObservation {
    match case.kind {
        CaseKind::Pass => measured(passing_evidence(case.gate)),
        CaseKind::Fail => measured(failing_evidence(case.gate)),
        CaseKind::StagedPass => measured(CoreEvidence::MergeIntegrity(staged_merge("1", "2"))),
        CaseKind::SameTreePass => measured(CoreEvidence::MergeIntegrity(compared_merge(
            "1",
            "1",
            MergeTreeCompletion::Clean,
            Vec::new(),
        ))),
        CaseKind::ConflictedFail => measured(CoreEvidence::MergeIntegrity(compared_merge(
            "1",
            "2",
            MergeTreeCompletion::Conflicted,
            vec!["spice/src/lib.rs".to_owned()],
        ))),
        CaseKind::UnnamedConflictFail => measured(CoreEvidence::MergeIntegrity(compared_merge(
            "1",
            "2",
            MergeTreeCompletion::Conflicted,
            Vec::new(),
        ))),
        CaseKind::PeerFail => measured(CoreEvidence::RepoDocs(
            RepoDocsEvidence::new(vec![
                DocMeasurement::new("AGENTS.md", 21, 20, true).expect("doc measurement"),
            ])
            .expect("repo-doc evidence"),
        )),
        CaseKind::UnlimitedPass => measured(CoreEvidence::RepoDocs(
            RepoDocsEvidence::new(vec![
                DocMeasurement::unlimited("AGENTS.md", u64::MAX).expect("doc measurement"),
            ])
            .expect("repo-doc evidence"),
        )),
        CaseKind::Unavailable => CoreObservation::Unavailable {
            reason: "git executable was not admitted".to_owned(),
        },
        CaseKind::Unsupported => CoreObservation::Unsupported {
            reason: "predecessor-only package-root grammar".to_owned(),
            recovery: concat!(
                "release 0.31.0 owns migration; declare current immutable ",
                "shape inputs"
            )
            .to_owned(),
        },
        CaseKind::Unmeasured => CoreObservation::Unmeasured {
            reason: "selected tree content was not decoded".to_owned(),
        },
        CaseKind::TimedOut => CoreObservation::TimedOut { milliseconds: 250 },
        CaseKind::Skipped => CoreObservation::Skipped {
            reason: "no claim store is configured for this repository".to_owned(),
        },
        CaseKind::ShapeSkipped => CoreObservation::Skipped {
            reason: "no package roots are selected by current shape policy".to_owned(),
        },
    }
}

fn battery_for(tree: &Digest, target: GateId, observation: CoreObservation) -> Vec<CoreArmInput> {
    CORE_GATE_IDS
        .into_iter()
        .map(|id| {
            let value = if id == target {
                observation.clone()
            } else {
                measured(passing_evidence(id))
            };
            arm(id, tree.clone(), value)
        })
        .collect()
}

fn passing_arms(tree: &Digest) -> Vec<CoreArmInput> {
    CORE_GATE_IDS
        .into_iter()
        .map(|id| arm(id, tree.clone(), measured(passing_evidence(id))))
        .collect()
}

fn arm(id: GateId, tree: Digest, observation: CoreObservation) -> CoreArmInput {
    CoreArmInput::new(id, tree, provenance(id), observation).expect("valid fixture arm")
}

fn provenance(id: GateId) -> CoreProvenance {
    let predecessor = declaration(id).predecessor();
    CoreProvenance::new(
        predecessor.release(),
        predecessor.source(),
        predecessor.observation(),
    )
    .expect("valid compiled predecessor provenance")
}

fn declaration(id: GateId) -> gate::CoreArmDeclaration {
    *core_declarations()
        .iter()
        .find(|declaration| declaration.id() == id)
        .expect("core identity has a declaration")
}

fn passing_evidence(id: GateId) -> CoreEvidence {
    match id {
        GateId::MergeIntegrity => CoreEvidence::MergeIntegrity(MergeIntegrityEvidence::NoMerge),
        GateId::PlanPhase => CoreEvidence::PlanPhase(PlanPhaseEvidence::NoActiveClaim),
        GateId::RepoShape => CoreEvidence::RepoShape(RepoShapeEvidence::new(Vec::new())),
        GateId::Staging => CoreEvidence::Staging(
            StagingEvidence::new(Vec::<String>::new()).expect("empty staging evidence"),
        ),
        GateId::RepoDocs => CoreEvidence::RepoDocs(
            RepoDocsEvidence::new(vec![
                DocMeasurement::new("AGENTS.md", 10, 20, false).expect("doc measurement"),
            ])
            .expect("repo-doc evidence"),
        ),
        GateId::ConfigKeyValidity => {
            CoreEvidence::ConfigKeyValidity(ConfigKeyEvidence::new(Vec::new()))
        }
        GateId::ConfigFalseDisable => {
            CoreEvidence::ConfigFalseDisable(matching_inventory(&["policy", "pre_commit_builtins"]))
        }
        GateId::ConfigTrackedTrust => {
            CoreEvidence::ConfigTrackedTrust(matching_inventory(&["policy", "pre_commit"]))
        }
        _ => panic!("{id} is not a core gate"),
    }
}

fn failing_evidence(id: GateId) -> CoreEvidence {
    match id {
        GateId::MergeIntegrity => CoreEvidence::MergeIntegrity(compared_merge(
            "1",
            "2",
            MergeTreeCompletion::Clean,
            Vec::new(),
        )),
        GateId::PlanPhase => CoreEvidence::PlanPhase(PlanPhaseEvidence::Active {
            task: TaskHandle::from_str("CORE-1kJWlbWK").expect("task handle"),
            phase: Phase::from_str("plan").expect("phase"),
        }),
        GateId::RepoShape => CoreEvidence::RepoShape(RepoShapeEvidence::new(vec![finding(
            "name-cluster",
            Some("spice/src"),
            "four modules share an affix",
        )])),
        GateId::Staging => CoreEvidence::Staging(
            StagingEvidence::new(["gate/src/core.rs"]).expect("staging evidence"),
        ),
        GateId::RepoDocs => CoreEvidence::RepoDocs(
            RepoDocsEvidence::new(vec![
                DocMeasurement::new("AGENTS.md", 21, 20, false).expect("doc measurement"),
            ])
            .expect("repo-doc evidence"),
        ),
        GateId::ConfigKeyValidity => {
            CoreEvidence::ConfigKeyValidity(ConfigKeyEvidence::new(vec![finding(
                "unknown-key",
                None::<String>,
                "policy.unknown is not declared",
            )]))
        }
        GateId::ConfigFalseDisable => {
            CoreEvidence::ConfigFalseDisable(drifted_inventory(&["policy", "pre_commit_builtins"]))
        }
        GateId::ConfigTrackedTrust => {
            CoreEvidence::ConfigTrackedTrust(drifted_inventory(&["policy", "pre_commit"]))
        }
        _ => panic!("{id} is not a core gate"),
    }
}

fn matching_inventory(path: &[&str]) -> ConfigInventoryEvidence {
    let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
    ConfigInventoryEvidence::new(vec![path.clone()], vec![path], Vec::new())
        .expect("matching inventory")
}

fn drifted_inventory(path: &[&str]) -> ConfigInventoryEvidence {
    let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
    ConfigInventoryEvidence::new(vec![path], Vec::new(), Vec::new()).expect("drifted inventory")
}

fn finding(code: &str, path: Option<impl Into<String>>, detail: &str) -> CoreFinding {
    CoreFinding::new(code, path, detail).expect("valid fixture finding")
}

fn fixture_tree() -> Digest {
    Digest::compute(Domain::GateTree, b"core predecessor parity fixture")
}

fn git_tree(digit: &str) -> String {
    digit.repeat(40)
}

fn staged_merge(head: &str, staged: &str) -> MergeIntegrityEvidence {
    MergeIntegrityEvidence::StagedContent {
        head_tree: git_tree(head),
        staged_tree: git_tree(staged),
    }
}

fn compared_merge(
    head: &str,
    expected: &str,
    completion: MergeTreeCompletion,
    conflicted_paths: Vec<String>,
) -> MergeIntegrityEvidence {
    MergeIntegrityEvidence::Compared {
        head_tree: git_tree(head),
        expected_tree: git_tree(expected),
        completion,
        conflicted_paths,
    }
}

fn acquired_input(inputs: &CoreInputs, id: GateId) -> &CoreArmInput {
    inputs
        .arms()
        .iter()
        .find(|arm| arm.id() == id)
        .unwrap_or_else(|| panic!("{id} is missing from an acquired battery"))
}

#[test]
fn acquisition_measures_every_compiled_provider_the_core_fixtures_assert() {
    let repository = contract_repository("core-acquire-clean");
    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);

    for id in [
        GateId::MergeIntegrity,
        GateId::PlanPhase,
        GateId::Staging,
        GateId::ConfigKeyValidity,
    ] {
        let acquired = acquired_arm(&report, id);
        assert_eq!(
            acquired.observation(),
            &measured(passing_evidence(id)),
            "{id} acquisition disagrees with the asserted core fixture evidence"
        );
        assert_eq!(
            acquired.standing(),
            CoreStanding::Passed,
            "{id}: {}",
            acquired.diagnostic()
        );
    }

    repository.remove();
}

#[test]
fn a_partially_staged_file_is_acquired_from_the_captured_repository() {
    let repository = contract_repository("core-acquire-partial");
    repository.write("src/app.rs", "fn main() {\n    // staged\n}\n");
    git(&repository, &["add", "src/app.rs"]);
    repository.write("src/app.rs", "fn main() {\n    // unstaged\n}\n");

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let staging = acquired_arm(&report, GateId::Staging);

    assert_eq!(
        staging.observation(),
        &measured(CoreEvidence::Staging(
            StagingEvidence::new(["src/app.rs"]).expect("partial staging evidence")
        )),
        "acquisition must name the file that is both staged and unstaged"
    );
    assert_eq!(staging.standing(), CoreStanding::Failed);

    repository.remove();
}

/// Staging evidence names the repository as the capture found it, never later.
///
/// The capture is clean. The index and the worktree are then mutated into the
/// exact state the arm exists to fail on, and acquisition still crosses the
/// clean observation under the captured digest. The second capture is the
/// control: it proves the mutation is real, observable, and reported by this
/// very code path, so the first result is a binding rather than a scanner that
/// never sees anything. Both captures advertise one digest, because the
/// selected tree is immutable, which leaves the capture moment as the only
/// thing that distinguishes them.
#[test]
fn a_repository_mutated_after_capture_cannot_cross_later_staging_evidence() {
    let repository = contract_repository("core-acquire-mutated");
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");
    let captured = CoreSource::capture(
        &tree,
        &contract,
        ClaimCapture::Idle,
        ActorCapture::Absent,
        &[],
    )
    .expect("capture one clean core source");

    repository.write("src/app.rs", "fn main() {\n    // staged\n}\n");
    git(&repository, &["add", "src/app.rs"]);
    repository.write("src/app.rs", "fn main() {\n    // unstaged\n}\n");

    let inputs = acquire_core(&captured, &contract).expect("acquire one complete core battery");
    let staging = acquired_input(&inputs, GateId::Staging);
    assert_eq!(
        staging.observation(),
        &measured(CoreEvidence::Staging(
            StagingEvidence::new(Vec::<String>::new()).expect("clean staging evidence")
        )),
        "a repository mutated after capture reached acquisition"
    );
    assert_eq!(
        staging.tree(),
        captured.digest(),
        "the staging arm is labelled with the digest its observation was taken under"
    );

    let later = CoreSource::capture(
        &tree,
        &contract,
        ClaimCapture::Idle,
        ActorCapture::Absent,
        &[],
    )
    .expect("capture the same tree over the mutated repository");
    let later_inputs = acquire_core(&later, &contract).expect("acquire the later battery");
    assert_eq!(
        acquired_input(&later_inputs, GateId::Staging).observation(),
        &measured(CoreEvidence::Staging(
            StagingEvidence::new(["src/app.rs"]).expect("partial staging evidence")
        )),
        "the mutation this test relies on is not observable at all"
    );
    assert_eq!(
        later.digest(),
        captured.digest(),
        "one immutable tree keeps one identity across both captures"
    );

    repository.remove();
}

#[test]
fn a_captured_claim_reaches_the_plan_phase_arm_without_the_gate_reading_a_journal() {
    let repository = contract_repository("core-acquire-claim");
    let task = TaskHandle::from_str("CORE-1kJWlbWK").expect("task handle");
    let phase = Phase::from_str("plan").expect("phase");
    let report = acquired(
        &repository,
        ClaimCapture::Active {
            task: task.clone(),
            phase: phase.clone(),
        },
        ActorCapture::Absent,
    );

    assert_eq!(
        acquired_arm(&report, GateId::PlanPhase).observation(),
        &measured(CoreEvidence::PlanPhase(PlanPhaseEvidence::Active {
            task,
            phase
        })),
    );

    repository.remove();
}

#[test]
fn a_repository_without_a_claim_store_skips_plan_phase_rather_than_measuring_it() {
    let repository = contract_repository("core-acquire-no-store");
    let report = acquired(
        &repository,
        ClaimCapture::Absent {
            reason: "no claim store is configured for this repository".to_owned(),
        },
        ActorCapture::Absent,
    );
    let plan = acquired_arm(&report, GateId::PlanPhase);

    assert_eq!(plan.standing(), CoreStanding::Skipped);
    assert!(
        plan.evidence().is_none(),
        "an absent store measured nothing"
    );

    repository.remove();
}

#[test]
fn a_disappeared_selected_tree_refuses_capture_with_a_typed_core_input_error() {
    let repository = contract_repository("core-acquire-disappeared");
    let tree = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the captured index");
    let contract = Contract::load(tree.root()).expect("load the captured contract");

    let displaced = tree.root().with_file_name("displaced-root");
    fs::rename(tree.root(), &displaced).expect("displace the selected tree root");
    let refused = CoreSource::capture(
        &tree,
        &contract,
        ClaimCapture::Idle,
        ActorCapture::Absent,
        &[],
    );

    let error = refused.expect_err("a disappeared selected tree cannot yield one identity");
    assert!(
        error.to_string().contains("core source"),
        "capture must name the source it could not complete: {error}"
    );

    repository.remove();
}

#[test]
fn an_empty_merge_is_acquired_from_the_repository_and_refused_by_the_engine() {
    let repository = contract_repository("core-acquire-empty-merge");
    git(&repository, &["checkout", "-q", "-b", "side"]);
    repository.write("src/side.rs", "pub fn side() {}\n");
    git_commit(&repository, "side work");

    git(&repository, &["checkout", "-q", "main"]);
    repository.write("src/main.rs", "pub fn main_work() {}\n");
    git_commit(&repository, "main work");

    let output = git_output(&repository, &["merge", "--no-commit", "--no-ff", "side"]);
    assert!(
        output.status.success(),
        "the fixture merge must succeed cleanly: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(repository.path(".git/MERGE_HEAD").exists());
    git(&repository, &["read-tree", "--reset", "-u", "HEAD"]);

    let report = acquired(&repository, ClaimCapture::Idle, ActorCapture::Absent);
    let merge = acquired_arm(&report, GateId::MergeIntegrity);

    let head_tree = git(&repository, &["rev-parse", "HEAD^{tree}"]);
    let CoreObservation::Measured {
        evidence:
            CoreEvidence::MergeIntegrity(MergeIntegrityEvidence::Compared {
                head_tree: acquired_head,
                expected_tree,
                completion,
                conflicted_paths,
            }),
    } = merge.observation()
    else {
        panic!(
            "an in-progress merge must be compared, not {:?}",
            merge.observation()
        );
    };
    assert_eq!(
        acquired_head, &head_tree,
        "acquisition read the real HEAD tree"
    );
    assert_eq!(*completion, MergeTreeCompletion::Clean);
    assert!(conflicted_paths.is_empty());
    assert_ne!(
        expected_tree, &head_tree,
        "git merge-tree must compute the merge this reset index would discard"
    );

    assert_eq!(merge.standing(), CoreStanding::Failed);
    assert!(merge.diagnostic().contains("empty merge refused"));

    repository.remove();
}
