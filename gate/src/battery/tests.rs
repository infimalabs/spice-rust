use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::fixtures;
use super::*;
use crate::RulePolicy;
use crate::manifest::{GateId, OriginLayer, PREDECESSOR_GATE_RELEASE};
use crate::state::{
    ActorInputs, ActorResolution, BoundDecision, BoundRule, CurrentState, PredecessorSources,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn canonical_manifest_is_the_exact_25_arm_compiled_contract() {
    let manifest = BatteryManifest::canonical().expect("canonical manifest");
    assert_eq!(manifest.version(), GATE_BATTERY_VERSION);
    assert_eq!(manifest.predecessor_release(), PREDECESSOR_GATE_RELEASE);
    assert_eq!(manifest.arms().len(), 25);

    for (position, (arm, expected)) in manifest.arms().iter().zip(GateId::ALL).enumerate() {
        assert_eq!(arm.predecessor(), expected.predecessor_name());
        let ArmDisposition::Compiled {
            gate_id,
            family,
            declaration,
        } = arm.disposition();
        assert_eq!(*gate_id, expected, "canonical arm {position}");
        assert_eq!(
            (*family, declaration.clone()),
            declaration_identity(expected).expect("compiled identity")
        );
    }
}

#[test]
fn manifest_rejects_every_noncanonical_shape_and_runtime_escape() {
    let source = CANONICAL_BATTERY_MANIFEST;
    let last = source.rfind("[[arms]]").expect("last arm marker");
    let missing = source[..last].to_owned();
    let duplicate = source.replacen(
        "predecessor = \"plan-phase\"",
        "predecessor = \"merge-integrity\"",
        1,
    );
    let reordered = source
        .replacen(
            "predecessor = \"merge-integrity\"",
            "predecessor = \"temporary-arm\"",
            1,
        )
        .replacen(
            "predecessor = \"plan-phase\"",
            "predecessor = \"merge-integrity\"",
            1,
        )
        .replacen(
            "predecessor = \"temporary-arm\"",
            "predecessor = \"plan-phase\"",
            1,
        );
    let unknown = source.replacen(
        "gate_id = \"merge-integrity\"",
        "gate_id = \"foreign-gate\"",
        1,
    );
    let shell_backed = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"shell-backed\"",
        1,
    );
    let indirect = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"indirect\"",
        1,
    );
    let alternate_runtime = source.replacen(
        "disposition = \"compiled\"",
        "disposition = \"alternate-runtime\"",
        1,
    );
    // A complete, internally valid refusal arm: every field the deleted
    // `migration-refusal` disposition required, including the owning release its
    // own validator demanded. This decoded before that disposition was deleted,
    // so the case fails the moment the variant returns as an untested path.
    let migration_refusal = source.replacen(
        concat!(
            "disposition = \"compiled\"\n",
            "gate_id = \"merge-integrity\"\n",
            "family = \"core\"\n",
            "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\""
        ),
        concat!(
            "disposition = \"migration-refusal\"\n",
            "source_field = [\"battery\", \"merge-integrity\"]\n",
            "owning_release = \"0.31.0\"\n",
            "reason = \"predecessor arm has no compiled successor\"\n",
            "recovery = \"run the 0.31.0 battery for this arm\""
        ),
        1,
    );
    let unknown_shell_field = source.replacen(
        "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\"",
        concat!(
            "declaration = \"gate::core::merge-integrity::merge-tree-comparison-v1\"\n",
            "shell_command = \"legacy-check\""
        ),
        1,
    );

    for (name, candidate) in [
        ("missing", missing),
        ("duplicate", duplicate),
        ("reordered", reordered),
        ("unknown", unknown),
        ("shell-backed", shell_backed),
        ("indirect", indirect),
        ("alternate-runtime", alternate_runtime),
        ("migration-refusal", migration_refusal),
        ("unknown-shell-field", unknown_shell_field),
    ] {
        assert_ne!(
            candidate, source,
            "{name} candidate must differ from the canonical manifest to test anything"
        );
        assert!(
            BatteryManifest::decode(&candidate).is_err(),
            "{name} manifest must be refused"
        );
    }
}

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

#[test]
fn partial_and_reordered_vectors_cannot_decode_as_complete() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");

    assert_refusals(vec![
        (
            "complete battery report requires 25 results, received 24",
            mutate(&value, |candidate| {
                candidate["results"]
                    .as_array_mut()
                    .expect("result array")
                    .pop();
            }),
        ),
        (
            "battery result 0 is missing or reordered",
            mutate(&value, |candidate| {
                candidate["results"]
                    .as_array_mut()
                    .expect("result array")
                    .swap(0, 1);
            }),
        ),
    ]);
}

#[test]
fn decoded_run_context_cannot_be_forged() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");
    assert!(value["context"]["latches"][0].get("function").is_none());

    assert_refusals(vec![
        (
            "battery actor must be an explicit task-session or active-actor identity",
            mutate(&value, |candidate| {
                candidate["context"]["actor"]["source"] = serde_json::json!("candidate-path")
            }),
        ),
        (
            "bound evidence flags disagree with its limits or latch transition",
            mutate(&value, |candidate| {
                candidate["context"]["bounds"][0]["active_limit"] = serde_json::json!(999)
            }),
        ),
        (
            "bound evidence flags disagree with its limits or latch transition",
            mutate(&value, |candidate| {
                candidate["context"]["bounds"][0]["measured"] = serde_json::json!(999)
            }),
        ),
        (
            "bound candidate path must already be canonical",
            mutate(&value, |candidate| {
                candidate["context"]["bounds"][0]["actor_inputs"]["candidate_path"] =
                    serde_json::json!("./src/lib.rs")
            }),
        ),
        (
            "invalid latch key shape",
            mutate(&value, |candidate| {
                candidate["context"]["latches"][0]["function"] = serde_json::json!("forged")
            }),
        ),
        (
            "battery context repeats configuration key",
            mutate(&value, |candidate| {
                let first = candidate["context"]["configuration"][0].clone();
                candidate["context"]["configuration"]
                    .as_array_mut()
                    .expect("configuration array")
                    .push(first);
            }),
        ),
        (
            "battery context repeats tool evidence",
            mutate(&value, |candidate| {
                let first = candidate["context"]["tools"][0].clone();
                candidate["context"]["tools"]
                    .as_array_mut()
                    .expect("tool array")
                    .push(first);
            }),
        ),
    ]);
}

#[test]
fn decoded_family_evidence_cannot_be_forged() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");
    assert!(value["results"][0].get("provenance").is_none());

    assert_refusals(vec![
        (
            "invalid core evidence",
            mutate(&value, |candidate| {
                candidate["results"][0]["evidence"]["report"]["standing"] =
                    serde_json::json!("failed")
            }),
        ),
        (
            "core arm evidence belongs to a different immutable tree",
            mutate(&value, |candidate| {
                candidate["results"][0]["evidence"]["report"]["tree"] = serde_json::json!(
                    Digest::compute(Domain::GateTree, b"another fixture tree").as_str()
                )
            }),
        ),
        (
            "invalid toolchain evidence",
            mutate(&value, |candidate| {
                candidate["results"][8]["evidence"]["report"]["configuration_source"] =
                    serde_json::json!("")
            }),
        ),
        (
            "arm evidence belongs to a different compiled gate or family",
            mutate(&value, |candidate| {
                candidate["results"][8]["evidence"] = candidate["results"][9]["evidence"].clone();
                candidate["results"][8]["reason"] = candidate["results"][9]["reason"].clone();
            }),
        ),
        (
            "invalid toolchain evidence",
            mutate(&value, |candidate| {
                candidate["results"][8]["evidence"]["report"]["tools"] = serde_json::json!([])
            }),
        ),
        (
            "invalid metric evidence",
            mutate(&value, |candidate| {
                candidate["results"][16]["evidence"]["report"]["metrics"] = serde_json::json!([])
            }),
        ),
        (
            "invalid metric evidence",
            mutate(&value, |candidate| {
                candidate["results"][16]["evidence"]["report"]["metrics"][0]["state"]["measurement"]
                    ["value"] = serde_json::json!(999)
            }),
        ),
        (
            "invalid hygiene evidence",
            mutate(&value, |candidate| {
                candidate["results"][20]["evidence"]["report"]["evidence"] = serde_json::json!([])
            }),
        ),
        (
            "arm state or reason disagrees with its retained family evidence",
            mutate(&value, |candidate| {
                candidate["results"][0]["state"] = serde_json::json!("failed")
            }),
        ),
    ]);
}

#[test]
fn admitted_skip_provenance_and_reason_cannot_be_forged() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::nonpassing(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("typed nonpass report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");

    assert_refusals(vec![
        (
            "arm provenance disagrees with its declaration-owned skip evidence",
            mutate(&value, |candidate| {
                candidate["results"][20]["provenance"]["source"] = serde_json::json!("forged")
            }),
        ),
        (
            "arm state or reason disagrees with its retained family evidence",
            mutate(&value, |candidate| {
                candidate["results"][21]["reason"] = serde_json::json!("forged exclusion")
            }),
        ),
    ]);
}

#[test]
fn metric_arms_cannot_assemble_mixed_terminal_evidence() {
    let fixture = Fixture::new();
    let ArmEvidence::Metrics { report } = fixtures::passing(&fixture.source)[16].clone() else {
        panic!("the seventeenth compiled arm retains structural metric evidence")
    };
    let mut metrics = report.metrics().to_vec();
    metrics[1] = crate::metrics::MetricEvidence::new(
        metrics[1].metric(),
        crate::metrics::MetricKey::Arm,
        crate::metrics::EvidenceState::Unavailable {
            reason: "structural parser was not admitted".to_owned(),
        },
    )
    .expect("unavailable metric evidence");
    let error = crate::metrics::StructuralArmEvidence::new(report.gate_id(), metrics)
        .expect_err("one arm cannot pair a measured key with an unmeasured arm key");
    assert_eq!(
        error.to_string(),
        "file-shape metric keys differ: left={Path(\"src/app.rs\")} right={Arm}"
    );
}

#[test]
fn execution_boundary_contains_no_shell_or_alternate_runtime() {
    let runner = concat!(
        include_str!("../battery.rs"),
        include_str!("context.rs"),
        include_str!("execution.rs"),
        include_str!("family_evidence.rs"),
        include_str!("manifest_contract.rs"),
    );
    for forbidden in [
        "std::process::Command",
        "Command::new",
        "python -m",
        "task export",
        "sh -c",
    ] {
        assert!(
            !runner.contains(forbidden),
            "battery runner must not contain {forbidden:?}"
        );
    }
}

struct RecordingExecutor {
    calls: Vec<GateId>,
    evidence: Vec<ArmEvidence>,
    refusal: Option<usize>,
    invalid: Option<usize>,
}

impl RecordingExecutor {
    fn all_passed(source: &Digest) -> Self {
        Self::with_evidence(fixtures::passing(source))
    }

    fn nonpassing(source: &Digest) -> Self {
        Self::with_evidence(fixtures::nonpassing(source))
    }

    fn with_evidence(evidence: Vec<ArmEvidence>) -> Self {
        Self {
            calls: vec![],
            evidence,
            refusal: None,
            invalid: None,
        }
    }

    fn faulting(source: &Digest, refusal: usize, invalid: usize) -> Self {
        Self {
            refusal: Some(refusal),
            invalid: Some(invalid),
            ..Self::all_passed(source)
        }
    }
}

impl BatteryExecutor for RecordingExecutor {
    fn execute(
        &mut self,
        arm: &CompiledArm,
        inputs: &BatteryInputs<'_>,
    ) -> Result<ArmExecution, BatteryError> {
        assert_eq!(inputs.source().as_str().len(), 64);
        assert_eq!(inputs.actor().value(), "fixture-actor");
        assert_eq!(inputs.scope().name(), "selected-index");
        assert_eq!(inputs.configuration().len(), 1);
        assert!(!inputs.policy().document().rules().is_empty());
        assert!(
            inputs
                .policy()
                .document()
                .rules()
                .iter()
                .any(|rule| rule.id().as_str().starts_with("legacy-rule:"))
        );
        assert_eq!(inputs.state().latches().len(), 1);
        assert_eq!(inputs.bounds().len(), 1);
        assert_eq!(inputs.tools().len(), 1);

        self.calls.push(arm.gate_id());
        if self.refusal == Some(arm.position()) {
            return Err(BatteryError::new("provider\nlost"));
        }
        let evidence = self.evidence[arm.position()].clone();
        let state = evidence.state()?;
        let provenance = evidence.provenance()?;
        let reason = evidence.reason()?;
        let execution = ArmExecution::new(state, reason, provenance, evidence)?;
        if self.invalid == Some(arm.position()) {
            let mut value = serde_json::to_value(execution).expect("execution JSON");
            value["reason"] = serde_json::json!("forged\nreason");
            return Ok(serde_json::from_value(value).expect("structurally typed execution"));
        }
        Ok(execution)
    }
}

fn assert_family_evidence(evidence: &ArmEvidence, source: &Digest) {
    match evidence {
        ArmEvidence::Core { report } => {
            assert_eq!(report.tree(), source);
            assert!(report.evidence().is_some());
        }
        ArmEvidence::Toolchain { report } => {
            assert_eq!(report.outcome, crate::toolchain::ArmOutcome::Passed);
            assert!(!report.configuration_source.is_empty());
            assert!(!report.configuration_field.is_empty());
            assert!(!report.applicability.is_empty());
            let declaration = crate::toolchain::TOOLCHAIN_ARMS
                .into_iter()
                .find(|arm| arm.id() == report.gate_id)
                .expect("toolchain declaration");
            assert_eq!(report.tools.len(), declaration.tools().len());
            for (evidence, required) in report.tools.iter().zip(declaration.tools()) {
                let crate::toolchain::ToolEvidence::Identified {
                    name,
                    requested_program,
                    version,
                    digest,
                    ..
                } = evidence
                else {
                    panic!("passing toolchain evidence must identify every declared tool")
                };
                assert_eq!(name, required.name());
                assert_eq!(requested_program, required.program());
                assert_eq!(version, required.exact_version());
                assert!(!digest.is_empty());
            }
        }
        ArmEvidence::Metrics { report } => {
            assert!(!report.metrics().is_empty());
            for metric in report.metrics() {
                let crate::metrics::EvidenceState::Measured { measurement } = metric.state() else {
                    panic!("passing metric evidence must be measured")
                };
                if matches!(
                    metric.metric(),
                    crate::metrics::MetricId::FileLines
                        | crate::metrics::MetricId::FileBytes
                        | crate::metrics::MetricId::RoutineCcn
                        | crate::metrics::MetricId::RoutineLength
                ) {
                    let rule = measurement.rule().expect("selected metric policy rule");
                    assert_eq!(rule.rule_id(), Some("fixture-source-rule"));
                    let latch = measurement.latch().expect("metric threshold and latch");
                    assert_eq!(latch.transition(), "clear");
                    assert!(!latch.over_limit());
                }
            }
        }
        ArmEvidence::Hygiene { report } => {
            assert_eq!(report.outcome, crate::hygiene::HygieneOutcome::Passed);
            assert!(!report.configuration.is_empty());
            assert!(!report.evidence.is_empty());
            assert!(!report.traversals.is_empty());
            assert!(report.evidence.iter().any(|evidence| matches!(
                evidence,
                crate::hygiene::AnalysisEvidence::PolicyDecision { .. }
            )));
            assert!(report.evidence.iter().any(|evidence| matches!(
                evidence,
                crate::hygiene::AnalysisEvidence::Analyzer(
                    crate::hygiene::AnalyzerEvidence::Identified { digest, .. }
                ) if !digest.is_empty()
            )));
        }
        ArmEvidence::ExecutorFailure { .. } => {
            panic!("passing fixture must retain family-owned evidence")
        }
    }
}

fn mutate(
    value: &serde_json::Value,
    change: impl FnOnce(&mut serde_json::Value),
) -> serde_json::Value {
    let mut changed = value.clone();
    change(&mut changed);
    changed
}

/// Every forged report must be refused by the exact invariant it violates.
///
/// A re-serialized candidate no longer carries canonical field order, so decode
/// would refuse all of them on bytes alone; naming the expected refusal is what
/// proves each mutation is actually caught by the contract it targets.
fn assert_refusals(candidates: Vec<(&str, serde_json::Value)>) {
    let mut unproven = Vec::new();
    for (invariant, candidate) in candidates {
        let bytes = serde_json::to_vec(&candidate).expect("candidate JSON");
        let error = BatteryReport::decode(&bytes).expect_err("forged report must be refused");
        if !error.detail().contains(invariant) {
            unproven.push(format!(
                "expected a refusal naming {invariant:?}, observed {:?}",
                error.detail()
            ));
        }
    }
    assert!(unproven.is_empty(), "{}", unproven.join("\n"));
}

struct Fixture {
    root: PathBuf,
    source: Digest,
    actor: ActorResolution,
    scope: BatteryScope,
    configuration: Vec<ConfigurationEvidence>,
    policy: RulePolicy,
    state: CurrentState,
    bounds: Vec<BoundDecision>,
    tools: Vec<BatteryToolEvidence>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-battery-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let worktree = root.join("worktree");
        let shared = root.join("shared");
        fs::create_dir_all(&worktree).expect("worktree state root");
        fs::create_dir_all(&shared).expect("shared state root");
        fs::write(
            worktree.join("file-loc-sticky.json"),
            br#"{"version":1,"paths":["src/lib.rs"]}"#,
        )
        .expect("predecessor latch");
        let sources = PredecessorSources::new(&worktree, &shared);
        let state = CurrentState::load(root.join("current.json"), &sources).expect("current state");
        let actor_inputs = ActorInputs::new("src/lib.rs")
            .expect("actor inputs")
            .with_active_actor("fixture-actor");
        let actor = actor_inputs.require_actor().expect("explicit actor");
        let bounds = vec![
            BoundRule::new("file-shape", "file-loc", 100, 150, false, true)
                .expect("bound rule")
                .decide(101, false, &actor_inputs, 5)
                .expect("bound decision"),
        ];
        let source = Digest::compute(Domain::GateTree, b"fixture selected tree");
        let configuration = vec![
            ConfigurationEvidence::new(
                "policy.test_paths",
                Digest::compute(Domain::ConfigExecutableSurface, b"tests"),
                "current",
                OriginLayer::Repository,
                "spice.toml",
                vec!["policy".to_owned(), "test_paths".to_owned()],
            )
            .expect("configuration provenance"),
        ];
        let policy = crate::migrate_predecessor_policy(
            "spice.toml",
            r#"
                [[policy.rules]]
                scopes = { paths = ["src/**"] }

                [policy.rules.file_loc]
                multiplier = 1.0
            "#,
        )
        .expect("compiled fixture policy");
        let tools = vec![
            BatteryToolEvidence::identified(
                "cargo",
                "cargo",
                "cargo fixture",
                Digest::compute(Domain::GateTool, b"fixture cargo"),
            )
            .expect("tool evidence"),
        ];
        Self {
            root,
            source,
            actor,
            scope: BatteryScope::new("selected-index", "compiled test selector").expect("scope"),
            configuration,
            policy,
            state,
            bounds,
            tools,
        }
    }

    fn inputs(&self) -> BatteryInputs<'_> {
        BatteryInputs::new(
            &self.source,
            &self.actor,
            &self.scope,
            &self.configuration,
            &self.policy,
            self.state.before(),
            &self.bounds,
            &self.tools,
        )
        .expect("battery inputs")
    }

    fn policy_digest(&self) -> Digest {
        Digest::compute(
            Domain::GatePolicy,
            &wire::encode(self.policy.document()).expect("policy JSON"),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove battery fixture");
    }
}
