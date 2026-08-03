use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        None,
        changes,
        hygiene,
        &mut record,
    )
    .expect("capture production battery source");
    let source = executor.source().clone();
    assert_ne!(executor.root(), repository);

    fs::write(repository.join("tests/test_main.py"), "def test_broken(:\n")
        .expect("move the original repository after capture");
    let inputs = fixture.inputs_for(&source);
    let report = run_battery(&inputs, &mut executor).expect("complete production battery");
    drop(executor);

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
    let hygiene_results: Vec<_> = report
        .results()
        .iter()
        .filter(|result| result.family() == ArmFamily::Hygiene)
        .collect();
    assert!(hygiene_results.iter().all(|result| {
        !matches!(
            result.evidence(),
            ArmEvidence::Hygiene { report }
                if report.outcome == crate::HygieneOutcome::ParseFailure
        )
    }));

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

/// `decode` validates every invariant before it compares bytes, which is what
/// lets each forged candidate above name the invariant it violates. That
/// ordering leaves the byte comparison itself as the only refusal no mutation
/// reaches, so it is proven here directly: a report that is semantically
/// perfect but reserialized in another key order must still be refused, because
/// the gate-set identity binds bytes rather than fields.
#[test]
fn a_semantically_valid_report_in_another_key_order_is_still_refused() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete report");
    let canonical = report.encode().expect("encode");
    BatteryReport::decode(&canonical).expect("canonical bytes decode");

    // serde_json::Map is a BTreeMap here, so a Value round trip sorts the
    // report's fields alphabetically instead of the order the struct declares.
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("report JSON");
    let reordered = serde_json::to_vec(&value).expect("candidate JSON");
    assert_ne!(
        reordered, canonical,
        "a Value round trip must actually reorder the report, or this proves nothing"
    );

    let error = BatteryReport::decode(&reordered).expect_err("reordered bytes must be refused");
    assert_eq!(error.detail(), "battery report bytes are not canonical");
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
        include_str!("receipt.rs"),
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

#[test]
fn a_minted_receipt_binds_every_identity_of_its_run() {
    let fixture = Fixture::new();
    let baseline = fixture_baseline("published floors");
    let (report, receipt) = minted(&fixture, &baseline);

    assert_eq!(receipt.version(), GATE_BATTERY_RECEIPT_VERSION);
    assert_eq!(
        receipt.manifest(),
        &Digest::compute(
            Domain::GateBatteryManifest,
            CANONICAL_BATTERY_MANIFEST.as_bytes()
        )
    );
    assert_eq!(receipt.gate_set(), report.gate_set());
    assert_eq!(receipt.tree(), &fixture.source);
    assert_eq!(receipt.policy(), &fixture.policy_digest());
    assert_eq!(receipt.state(), report.context().state());
    assert_eq!(receipt.bounds(), report.context().bounds());
    assert_eq!(receipt.baseline(), &baseline);
    assert_eq!(receipt.actor(), report.context().actor());
    assert_eq!(receipt.scope(), report.context().scope());
    assert_eq!(receipt.configuration(), report.context().configuration());
    assert_eq!(receipt.latches(), report.context().latches());
    assert_eq!(receipt.tools(), report.context().tools());
    assert_eq!(
        receipt
            .results()
            .iter()
            .map(BatteryArmResult::gate_id)
            .collect::<Vec<_>>(),
        GateId::ALL,
    );
    assert_eq!(
        receipt.report(),
        &Digest::compute(
            Domain::GateBatteryReport,
            &report.encode().expect("report JSON")
        )
    );
    assert_eq!(receipt.run(), &report);
    assert_eq!(
        supported_pairs(),
        [(GATE_BATTERY_RECEIPT_VERSION, receipt.gate_set().clone())],
        "this build must interpret exactly the one pair it mints and no other",
    );

    // The one admitted spelling, pinned exactly: a receipt field added,
    // renamed, retyped, or moved changes these leading bytes, so it changes the
    // receipt's identity instead of quietly passing as the same schema.
    let encoded = receipt.encode().expect("receipt JSON");
    let text = String::from_utf8(encoded).expect("canonical receipts are UTF-8");
    assert!(
        text.starts_with(&format!(
            "{{\"version\":{},\"manifest\":\"{}\",\"baseline\":\"{}\",\"report\":\"{}\",\"run\":{{",
            GATE_BATTERY_RECEIPT_VERSION,
            receipt.manifest(),
            baseline,
            receipt.report(),
        )),
        "receipt schema is not the one admitted spelling: {}",
        &text[..text.len().min(240)],
    );
}

#[test]
fn a_verified_receipt_is_byte_stable_across_repeated_verification() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");

    let first = BatteryReceipt::verify(&canonical).expect("canonical receipt verifies");
    let again = first.encode().expect("re-encoded receipt");
    let second = BatteryReceipt::verify(&again).expect("a verified receipt verifies again");

    assert_eq!(first, receipt);
    assert_eq!(second, receipt);
    assert_eq!(again, canonical);
    assert_eq!(second.encode().expect("re-encoded receipt"), canonical);
    assert_eq!(
        receipt.digest(),
        Digest::compute(Domain::GateBatteryReceipt, &canonical)
    );
}

/// A pair this build does not carry is evidence about foreign bytes and nothing
/// more, so verification hands the bytes back exactly as they arrived.
///
/// Dispatch runs before the canonical byte comparison, which is what lets these
/// re-serialized candidates reach the version boundary at all; a build that
/// compared bytes first would refuse every foreign schema as merely
/// noncanonical and never name the pair it observed.
#[test]
fn an_unknown_schema_or_gate_set_is_returned_unread() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");
    let foreign = Digest::compute(Domain::GateSet, b"a gate set this build never compiled");

    for (name, forged, version, gate_set) in [
        (
            "unknown schema",
            mutate(&value, |candidate| {
                candidate["version"] = serde_json::json!(GATE_BATTERY_RECEIPT_VERSION + 1);
            }),
            GATE_BATTERY_RECEIPT_VERSION + 1,
            receipt.gate_set().clone(),
        ),
        (
            "unknown gate set",
            mutate(&value, |candidate| {
                candidate["run"]["gate_set"] = serde_json::json!(foreign.to_string());
            }),
            GATE_BATTERY_RECEIPT_VERSION,
            foreign.clone(),
        ),
    ] {
        let bytes = serde_json::to_vec(&forged).expect("candidate JSON");
        let refusal = BatteryReceipt::verify(&bytes).expect_err("a foreign pair must be refused");
        let unsupported = refusal.unsupported().unwrap_or_else(|| {
            panic!("{name} must refuse at the version boundary, observed {refusal}")
        });
        assert_eq!(unsupported.version(), version, "{name}");
        assert_eq!(unsupported.gate_set(), &gate_set, "{name}");
        assert_eq!(unsupported.bytes(), bytes, "{name} must retain its bytes");
        assert_eq!(
            unsupported.digest(),
            &Digest::compute(Domain::GateBatteryReceipt, &bytes),
            "{name} identity must be the digest of the bytes that arrived",
        );
        let rendered = refusal.to_string();
        assert!(
            !rendered.contains(std::str::from_utf8(&bytes).expect("candidate is UTF-8")),
            "{name} refusal must not spell foreign bytes into a current message: {rendered}",
        );
        for (schema, digest) in supported_pairs() {
            assert!(
                rendered.contains(&format!("schema {schema} with gate set {digest}")),
                "{name} refusal must name every pair this build interprets: {rendered}",
            );
        }
    }
}

/// Every invariant is checked before the bytes are compared, which is what lets
/// each forged candidate above name the invariant it violates. That ordering
/// leaves the byte comparison itself as the only refusal no forgery reaches, so
/// it is proven here directly: a receipt that is semantically perfect but
/// reserialized in another key order must still be refused.
#[test]
fn a_semantically_valid_receipt_in_another_key_order_is_still_refused() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    BatteryReceipt::verify(&canonical).expect("canonical bytes verify");

    // serde_json::Map is a BTreeMap here, so a Value round trip sorts the
    // receipt's fields alphabetically instead of the order the struct declares.
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");
    let reordered = serde_json::to_vec(&value).expect("candidate JSON");
    assert_ne!(
        reordered, canonical,
        "a Value round trip must actually reorder the receipt, or this proves nothing"
    );

    let refusal = BatteryReceipt::verify(&reordered).expect_err("reordered bytes must be refused");
    assert!(refusal.unsupported().is_none());
    assert_eq!(
        refusal.to_string(),
        "battery receipt bytes are not canonical"
    );
}

/// The report's own gate-set invariant is deliberately absent from this matrix:
/// a receipt naming a foreign gate set never reaches it, because dispatch
/// refuses the pair first. That door is proven by
/// `an_unknown_schema_or_gate_set_is_returned_unread` and, for bare report
/// bytes, by `partial_and_reordered_vectors_cannot_decode_as_complete`.
#[test]
fn truncated_tampered_and_reordered_receipts_are_refused_by_the_invariant_they_break() {
    let fixture = Fixture::new();
    let (_, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let canonical = receipt.encode().expect("receipt JSON");
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("receipt JSON value");

    // Truncated bytes cannot name a pair at all, so they never reach dispatch.
    let truncated = &canonical[..canonical.len() / 2];
    let refusal = BatteryReceipt::verify(truncated).expect_err("truncated bytes must be refused");
    assert!(refusal.unsupported().is_none());
    assert!(
        refusal
            .to_string()
            .contains("battery receipt has no readable version envelope"),
        "truncation must be named as an unreadable envelope, observed {refusal}",
    );

    let incomplete = format!(
        "complete battery report requires {} results, received {}",
        GateId::ALL.len(),
        GateId::ALL.len() - 1
    );
    let candidates = vec![
        (
            "battery receipt names a foreign canonical manifest digest",
            mutate(&value, |candidate| {
                candidate["manifest"] = serde_json::json!(
                    Digest::compute(Domain::GateBatteryManifest, b"a manifest never checked in")
                        .to_string()
                );
            }),
        ),
        (
            "battery receipt report digest disagrees with its own run",
            mutate(&value, |candidate| {
                candidate["report"] = serde_json::json!(
                    Digest::compute(Domain::GateBatteryReport, b"a report never run").to_string()
                );
            }),
        ),
        (
            incomplete.as_str(),
            mutate(&value, |candidate| {
                candidate["run"]["results"]
                    .as_array_mut()
                    .expect("result vector")
                    .pop();
            }),
        ),
        (
            "battery result 0 is missing or reordered",
            mutate(&value, |candidate| {
                candidate["run"]["results"]
                    .as_array_mut()
                    .expect("result vector")
                    .swap(0, 1);
            }),
        ),
        (
            "arm state or reason disagrees with its retained family evidence",
            mutate(&value, |candidate| {
                candidate["run"]["results"][0]["reason"] = serde_json::json!("forged reason");
            }),
        ),
    ];
    for (invariant, forged) in candidates {
        let bytes = serde_json::to_vec(&forged).expect("candidate JSON");
        let refusal = BatteryReceipt::verify(&bytes).expect_err("a forged receipt must be refused");
        assert!(
            refusal.to_string().contains(invariant),
            "expected a refusal naming {invariant:?}, observed {refusal}",
        );
    }
}

/// A receipt proves internal agreement and current contract identity. It is not
/// a signature, and nothing cross-checks the caller-supplied baseline, so the
/// honest claim about a changed one is that it is a different receipt.
#[test]
fn a_receipt_bound_to_another_baseline_is_a_different_receipt() {
    let fixture = Fixture::new();
    let published = fixture_baseline("published floors");
    let (report, receipt) = minted(&fixture, &published);
    let successor = BatteryReceipt::mint(report, &fixture_baseline("tightened floors"));

    let bytes = successor.encode().expect("successor JSON");
    let verified = BatteryReceipt::verify(&bytes).expect("a rebaselined receipt is still valid");

    assert_eq!(verified, successor);
    assert_eq!(verified.results(), receipt.results());
    assert_eq!(verified.report(), receipt.report());
    assert_ne!(verified.baseline(), receipt.baseline());
    assert_ne!(
        verified.digest(),
        receipt.digest(),
        "a receipt bound to another baseline must not share the published one's identity",
    );
}

#[test]
fn an_interrupted_publication_leaves_the_previous_receipt_intact() {
    let fixture = Fixture::new();
    let (report, receipt) = minted(&fixture, &fixture_baseline("published floors"));
    let directory = fixture.root.join("receipts");
    fs::create_dir_all(&directory).expect("receipt directory");
    let destination = directory.join("battery-receipt.json");

    receipt.publish(&destination).expect("publish the receipt");
    let published = fs::read(&destination).expect("published receipt");
    assert_eq!(published, receipt.encode().expect("receipt JSON"));
    BatteryReceipt::verify(&published).expect("a published receipt verifies from disk");

    // A successor whose bytes are completely staged but never swapped in: the
    // exact shape of a run that ended between writing and publishing.
    let successor = BatteryReceipt::mint(report, &fixture_baseline("tightened floors"));
    let staging = atomic::Destination::new(&destination).expect("resolve destination");
    let mut staged = staging.stage().expect("stage the successor");
    staged
        .write_all(&successor.encode().expect("successor JSON"))
        .expect("write the staged bytes");
    drop(staged);

    assert_eq!(
        fs::read(&destination).expect("published receipt"),
        published,
        "an interrupted publication must leave the previous receipt exactly as it was",
    );
    let mut remaining: Vec<_> = fs::read_dir(&directory)
        .expect("read the receipt directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect();
    remaining.sort_unstable();
    assert_eq!(
        remaining,
        [destination.file_name().expect("receipt file name")],
        "an interrupted publication must leave no staged bytes behind",
    );
}

fn fixture_baseline(floors: &str) -> Digest {
    Digest::compute(Domain::GateBaseline, floors.as_bytes())
}

fn minted(fixture: &Fixture, baseline: &Digest) -> (BatteryReport, BatteryReceipt) {
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("complete battery");
    let receipt = BatteryReceipt::mint(report.clone(), baseline);
    (report, receipt)
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
        self.inputs_for(&self.source)
    }

    fn inputs_for<'a>(&'a self, source: &'a Digest) -> BatteryInputs<'a> {
        BatteryInputs::new(
            source,
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

fn production_repository(root: &Path) -> PathBuf {
    let repository = root.join("repository");
    fs::create_dir(&repository).expect("production repository root");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    fs::write(
        repository.join("spice.toml"),
        fs::read(workspace.join("spice.toml")).expect("workspace spice.toml"),
    )
    .expect("fixture spice.toml");
    fs::write(
        repository.join("baseline.toml"),
        fs::read(workspace.join("baseline.toml")).expect("workspace baseline.toml"),
    )
    .expect("fixture baseline.toml");
    fs::write(
        repository.join("pyproject.toml"),
        "[project]\nname = 'battery-fixture'\n",
    )
    .expect("fixture pyproject.toml");
    for directory in ["app", "tests", "src"] {
        fs::create_dir(repository.join(directory)).expect("fixture source directory");
    }
    fs::write(
        repository.join("app/__init__.py"),
        "from .main import entry\n",
    )
    .expect("fixture package");
    fs::write(
        repository.join("app/main.py"),
        "def entry():\n    return 1\n",
    )
    .expect("fixture production source");
    fs::write(
        repository.join("tests/test_main.py"),
        "from app.main import entry\n\ndef test_entry():\n    assert entry() == 1\n",
    )
    .expect("fixture test source");
    fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n").expect("fixture Rust source");

    fixture_git(&repository, &["init", "-q", "-b", "main"]);
    fixture_git(&repository, &["config", "core.excludesFile", "/dev/null"]);
    fixture_git(&repository, &["config", "user.name", "Battery Fixture"]);
    fixture_git(
        &repository,
        &["config", "user.email", "battery@example.invalid"],
    );
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "fixture"]);
    repository
}

fn fixture_git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(repository)
        .env_clear()
        .env("HOME", repository)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove battery fixture");
    }
}
