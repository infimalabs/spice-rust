use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Digest, Domain};

use super::*;
use crate::RulePolicy;
use crate::manifest::{GateId, OriginLayer};
use crate::policy::{BoundLimits, GlobalRatchet, PolicyDocument};
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
        } = arm.disposition()
        else {
            panic!("canonical arm {position} must be compiled");
        };
        assert_eq!(*gate_id, expected);
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
        ("unknown-shell-field", unknown_shell_field),
    ] {
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
    let mut executor = RecordingExecutor::all_passed();
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
        assert_eq!(result.gate_id(), Some(expected));
        assert_eq!(result.state(), BatteryState::Passed);
        assert_eq!(result.evidence().decisions().len(), 1);
        assert!(result.evidence().decisions()[0].decision());
        assert_eq!(
            result.evidence().decisions()[0].name(),
            format!("{expected}-decision")
        );
        assert_eq!(
            result.evidence().decisions()[0].source(),
            result.declaration().expect("declaration")
        );
        assert_eq!(result.evidence().measurements().len(), 1);
        assert_eq!(
            result.evidence().measurements()[0].value(),
            u64::try_from(position).expect("position fits u64")
        );
        assert_eq!(result.evidence().measurements()[0].unit(), "ordinal");
        assert_eq!(
            result.evidence().measurements()[0].source(),
            result.declaration().expect("declaration")
        );
    }

    let encoded = report.encode().expect("canonical report");
    assert_eq!(BatteryReport::decode(&encoded).expect("decode"), report);
}

#[test]
fn every_nonpass_state_refuses_without_erasing_later_results() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::states(vec![
        BatteryState::Passed,
        BatteryState::Failed,
        BatteryState::Unavailable,
        BatteryState::Unsupported,
        BatteryState::Unmeasured,
        BatteryState::TimedOut,
        BatteryState::ExplicitSkip,
        BatteryState::NotApplicable,
        BatteryState::Excluded,
    ]);
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
        assert_eq!(provenance.release(), "current");
        assert_eq!(provenance.source(), "fixture-declaration");
        assert_eq!(provenance.field(), &[result.predecessor().to_owned()]);
    }
}

#[test]
fn executor_refusals_and_invalid_evidence_stay_visible_and_total() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::faulting(3, 4);
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
        Some(GateId::PrivateInternals)
    );
}

#[test]
fn partial_and_reordered_vectors_cannot_decode_as_complete() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed();
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");

    let mut partial = value.clone();
    partial["results"]
        .as_array_mut()
        .expect("result array")
        .pop();
    let partial = serde_json::to_vec(&partial).expect("partial JSON");
    assert!(BatteryReport::decode(&partial).is_err());

    let mut reordered = value;
    reordered["results"]
        .as_array_mut()
        .expect("result array")
        .swap(0, 1);
    let reordered = serde_json::to_vec(&reordered).expect("reordered JSON");
    assert!(BatteryReport::decode(&reordered).is_err());
}

#[test]
fn decoded_context_and_family_evidence_cannot_be_forged() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed();
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let value: serde_json::Value =
        serde_json::from_slice(&report.encode().expect("encode")).expect("report JSON");
    assert!(value["results"][0].get("provenance").is_none());
    assert!(value["context"]["latches"][0].get("function").is_none());

    let mut candidates = Vec::new();
    candidates.push(mutate(&value, |candidate| {
        candidate["context"]["actor"]["source"] = serde_json::json!("candidate-path")
    }));
    candidates.push(mutate(&value, |candidate| {
        candidate["context"]["bounds"][0]["active_limit"] = serde_json::json!(999)
    }));
    candidates.push(mutate(&value, |candidate| {
        candidate["context"]["bounds"][0]["measured"] = serde_json::json!(999)
    }));
    candidates.push(mutate(&value, |candidate| {
        candidate["context"]["bounds"][0]["actor_inputs"]["candidate_path"] =
            serde_json::json!("./src/lib.rs")
    }));
    candidates.push(mutate(&value, |candidate| {
        candidate["context"]["latches"][0]["function"] = serde_json::json!("forged")
    }));
    candidates.push(mutate(&value, |candidate| {
        let first = candidate["context"]["configuration"][0].clone();
        candidate["context"]["configuration"]
            .as_array_mut()
            .expect("configuration array")
            .push(first);
    }));
    candidates.push(mutate(&value, |candidate| {
        let first = candidate["context"]["tools"][0].clone();
        candidate["context"]["tools"]
            .as_array_mut()
            .expect("tool array")
            .push(first);
    }));
    candidates.push(mutate(&value, |candidate| {
        candidate["results"][0]["evidence"]["decisions"][0]["name"] = serde_json::json!("")
    }));

    for candidate in candidates {
        assert!(
            BatteryReport::decode(&serde_json::to_vec(&candidate).expect("candidate JSON"))
                .is_err()
        );
    }
}

#[test]
fn execution_boundary_contains_no_shell_or_alternate_runtime() {
    let runner = concat!(
        include_str!("../battery.rs"),
        include_str!("context.rs"),
        include_str!("execution.rs"),
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
    states: Vec<BatteryState>,
    refusal: Option<usize>,
    invalid: Option<usize>,
}

impl RecordingExecutor {
    fn all_passed() -> Self {
        Self::states(vec![BatteryState::Passed])
    }

    fn states(states: Vec<BatteryState>) -> Self {
        Self {
            calls: vec![],
            states,
            refusal: None,
            invalid: None,
        }
    }

    fn faulting(refusal: usize, invalid: usize) -> Self {
        Self {
            refusal: Some(refusal),
            invalid: Some(invalid),
            ..Self::all_passed()
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
        assert_eq!(inputs.policy().document().rules().len(), 0);
        assert_eq!(inputs.state().latches().len(), 1);
        assert_eq!(inputs.bounds().len(), 1);
        assert_eq!(inputs.tools().len(), 1);

        self.calls.push(arm.gate_id());
        if self.refusal == Some(arm.position()) {
            return Err(BatteryError::new("provider\nlost"));
        }
        let state = self.states[arm.position() % self.states.len()];
        let provenance = state.requires_provenance().then(|| {
            ResultProvenance::new(
                "current",
                "fixture-declaration",
                vec![arm.predecessor().to_owned()],
            )
            .expect("skip provenance")
        });
        let decision = DecisionEvidence::new(
            format!("{}-decision", arm.gate_id()),
            state.admits(),
            arm.declaration(),
        )?;
        let measurement = MeasurementEvidence::new(
            "report-position",
            u64::try_from(arm.position()).expect("position fits u64"),
            "ordinal",
            arm.declaration(),
        )?;
        let execution = ArmExecution::new(
            state,
            format!("{} reached {state:?}", arm.gate_id()),
            provenance,
            family_evidence(arm.family(), decision, measurement),
        )?;
        if self.invalid == Some(arm.position()) {
            let mut value = serde_json::to_value(execution).expect("execution JSON");
            value["reason"] = serde_json::json!("forged\nreason");
            return Ok(serde_json::from_value(value).expect("structurally typed execution"));
        }
        Ok(execution)
    }
}

fn family_evidence(
    family: ArmFamily,
    decision: DecisionEvidence,
    measurement: MeasurementEvidence,
) -> ArmEvidence {
    let decisions = vec![decision];
    let measurements = vec![measurement];
    match family {
        ArmFamily::Core => ArmEvidence::Core {
            decisions,
            measurements,
        },
        ArmFamily::Toolchain => ArmEvidence::Toolchain {
            decisions,
            measurements,
        },
        ArmFamily::Metrics => ArmEvidence::Metrics {
            decisions,
            measurements,
        },
        ArmFamily::Hygiene => ArmEvidence::Hygiene {
            decisions,
            measurements,
        },
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
        let policy = RulePolicy::compile(PolicyDocument::predecessor(
            BoundLimits::PREDECESSOR,
            GlobalRatchet::PREDECESSOR,
            2,
            vec![],
        ))
        .expect("compiled policy");
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
