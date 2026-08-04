//! What a decoded report may claim, and what it may never claim.

use super::*;

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
            "complete battery report requires 26 results, received 25",
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
                candidate["results"][17]["evidence"]["report"]["metrics"] = serde_json::json!([])
            }),
        ),
        (
            "invalid metric evidence",
            mutate(&value, |candidate| {
                candidate["results"][17]["evidence"]["report"]["metrics"][0]["state"]["measurement"]
                    ["value"] = serde_json::json!(999)
            }),
        ),
        (
            "invalid hygiene evidence",
            mutate(&value, |candidate| {
                candidate["results"][21]["evidence"]["report"]["evidence"] = serde_json::json!([])
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
                candidate["results"][21]["provenance"]["source"] = serde_json::json!("forged")
            }),
        ),
        (
            "arm state or reason disagrees with its retained family evidence",
            mutate(&value, |candidate| {
                candidate["results"][22]["reason"] = serde_json::json!("forged exclusion")
            }),
        ),
    ]);
}

#[test]
fn metric_arms_cannot_assemble_mixed_terminal_evidence() {
    let fixture = Fixture::new();
    let ArmEvidence::Metrics { report } = fixtures::passing(&fixture.source)[17].clone() else {
        panic!("the eighteenth compiled arm retains structural metric evidence")
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
        include_str!("../../battery.rs"),
        include_str!("../context.rs"),
        include_str!("../execution.rs"),
        include_str!("../family_evidence.rs"),
        include_str!("../manifest_contract.rs"),
        include_str!("../receipt.rs"),
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
