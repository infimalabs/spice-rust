//! The refusal fold is bounded human guidance, never report authority.

use super::*;
use crate::metrics::{
    EvidenceState as MetricState, LatchBinding, MetricEvidence, MetricId, MetricKey,
    MetricMeasurement, ParserEvidence, ParserId, RuleBinding, StructuralArmEvidence,
};

#[test]
fn refusal_fold_lists_only_non_admitting_arms_in_canonical_order() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::nonpassing(&fixture.source);
    let report = run_battery(&inputs, &mut executor).expect("typed nonpass report");
    let canonical_before = report.encode().expect("canonical report");

    let fold = report.refusal_fold();
    let text = fold.as_str();
    assert_eq!(fold, report.refusal_fold());
    assert_eq!(fold.omitted_arms(), 0);
    assert_eq!(fold.omitted_findings(), 0);
    assert_eq!(
        report.encode().expect("canonical report after fold"),
        canonical_before
    );

    let refusing: Vec<_> = report
        .results()
        .iter()
        .filter(|result| !result.state().admits())
        .collect();
    assert!(
        text.starts_with(&format!(
            "non-authoritative battery refusal fold: {} non-admitting arms\n",
            refusing.len()
        )),
        "fold states exactly how many canonical arms refused"
    );
    let mut cursor = 0;
    for result in &refusing {
        let marker = format!(
            "arm position={} gate={} state={}",
            result.position(),
            result.gate_id(),
            result.state().as_str()
        );
        let offset = text[cursor..]
            .find(&marker)
            .unwrap_or_else(|| panic!("fold dropped refusing arm marker {marker}"));
        cursor += offset + marker.len();
        assert!(
            text.contains(&format!(
                "  reason={}",
                serde_json::to_string(result.reason()).expect("JSON reason")
            )),
            "fold dropped the retained reason for {}",
            result.gate_id()
        );
        assert!(
            text.contains(&format!(
                "  full-evidence: cargo xtask gate {}",
                result.gate_id()
            )),
            "fold dropped the exact selected-arm command for {}",
            result.gate_id()
        );
    }
    for result in report
        .results()
        .iter()
        .filter(|result| result.state().admits())
    {
        assert!(
            !text.contains(&format!(
                "arm position={} gate={} state=",
                result.position(),
                result.gate_id()
            )),
            "fold serialized passing-arm evidence for {}",
            result.gate_id()
        );
    }
    assert!(
        text.contains("arm position=0 gate=merge-integrity state=unavailable")
            && !text.contains("finding gate=merge-integrity"),
        "a non-metric refusal renders its state and reason without invented findings"
    );
    assert!(
        !text.contains("finding gate=complexity"),
        "passing metric evidence must not leak into a refusal fold"
    );
}

#[test]
fn metric_fold_names_every_breaching_key_value_and_bound() {
    let report = metric_breach_report();
    let fold = report.refusal_fold();
    let text = fold.as_str();

    assert!(
        text.contains(concat!(
            "finding gate=file-shape metric=file.lines ",
            "key=path=\"src/offending.rs\" state=failed unit=source-lines value=201 ",
            "base_limit=100 configured_ratchet_limit=150 ratchet_limit=150 ",
            "active_limit=150 latch_transition=latched"
        )),
        "fold must retain the concrete offending metric key and all latch limits: {text}"
    );
    assert!(
        !text.contains("metric=file.bytes"),
        "the paired passing measurement is evidence, but not refusal evidence"
    );
    assert!(
        text.contains(concat!(
            "finding gate=complexity metric=routine.ccn ",
            "key=path=\"src/offending.rs\" function=\"complicated\" state=failed ",
            "unit=cyclomatic-complexity value=20 base_limit=10 ",
            "configured_ratchet_limit=15 ratchet_limit=15 active_limit=15 ",
            "latch_transition=latched"
        )),
        "fold must retain a breaching routine path/function identity: {text}"
    );
    assert!(
        text.contains(concat!(
            "finding gate=magic-numbers metric=magic-number.regressions ",
            "key=path=\"src/magic.rs\" state=failed unit=regressions value=1 threshold=10"
        )),
        "fold must retain the threshold attached to a concrete finding: {text}"
    );
    assert_eq!(fold.omitted_arms(), 0);
    assert_eq!(fold.omitted_findings(), 0);
}

#[test]
fn fold_limit_is_deterministic_and_reports_omissions() {
    let oversized_reason = "x".repeat(BATTERY_REFUSAL_FOLD_MAX_BYTES * 2);
    let report = oversized_metric_report(oversized_reason);
    assert!(
        report.encode().expect("canonical oversized report").len() > BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "fixture must exceed the fold limit before it can prove it"
    );

    let first = report.refusal_fold();
    let second = report.refusal_fold();
    assert_eq!(first, second, "the same typed report has one stable fold");
    assert!(
        first.as_bytes().len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "fold must never bypass its explicit 65536-byte limit"
    );
    assert_eq!(first.omitted_arms(), 1);
    assert_eq!(first.omitted_findings(), 1);
    assert!(first.as_str().contains("gate=file-shape state=failed"));
    assert!(
        first
            .as_str()
            .contains("reason=<omitted from bounded fold>")
    );
    assert!(
        first
            .as_str()
            .contains("full-evidence: cargo xtask gate file-shape")
    );
    assert!(first.as_str().contains("omitted_arms=1 omitted_findings=1"));
}

fn metric_breach_report() -> BatteryReport {
    let fixture = Fixture::new();
    let mut evidence = fixtures::passing(&fixture.source);
    replace_metric_arm(
        &mut evidence,
        GateId::FileShape,
        vec![
            bounded_row(
                MetricId::FileLines,
                MetricKey::path("src/offending.rs").expect("fixture file path"),
                201,
                100,
                150,
                true,
            ),
            bounded_row(
                MetricId::FileBytes,
                MetricKey::path("src/offending.rs").expect("fixture file path"),
                500,
                1_000,
                1_500,
                false,
            ),
        ],
    );
    replace_metric_arm(
        &mut evidence,
        GateId::Complexity,
        vec![
            bounded_row(
                MetricId::RoutineCcn,
                MetricKey::function("src/offending.rs", "complicated")
                    .expect("fixture routine key"),
                20,
                10,
                15,
                true,
            ),
            bounded_row(
                MetricId::RoutineLength,
                MetricKey::function("src/offending.rs", "complicated")
                    .expect("fixture routine key"),
                30,
                100,
                150,
                false,
            ),
        ],
    );
    replace_metric_arm(&mut evidence, GateId::MagicNumbers, vec![magic_row()]);
    report_with_evidence(&fixture, evidence)
}

fn oversized_metric_report(reason: String) -> BatteryReport {
    let fixture = Fixture::new();
    let mut evidence = fixtures::passing(&fixture.source);
    replace_metric_arm(
        &mut evidence,
        GateId::FileShape,
        vec![
            MetricEvidence::new(
                MetricId::FileLines,
                MetricKey::path("src/oversized.rs").expect("fixture file path"),
                MetricState::Failed {
                    reason,
                    exit_code: Some(17),
                },
            )
            .expect("oversized failing metric evidence"),
            bounded_row(
                MetricId::FileBytes,
                MetricKey::path("src/oversized.rs").expect("fixture file path"),
                500,
                1_000,
                1_500,
                false,
            ),
        ],
    );
    report_with_evidence(&fixture, evidence)
}

fn replace_metric_arm(evidence: &mut [ArmEvidence], gate: GateId, metrics: Vec<MetricEvidence>) {
    let arm = evidence
        .iter_mut()
        .find(|evidence| evidence.gate_id() == gate)
        .unwrap_or_else(|| panic!("fixture contains {gate} evidence"));
    *arm = ArmEvidence::Metrics {
        report: StructuralArmEvidence::new(gate, metrics).expect("structural fixture evidence"),
    };
}

fn report_with_evidence(fixture: &Fixture, evidence: Vec<ArmEvidence>) -> BatteryReport {
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::with_evidence(evidence);
    run_battery(&inputs, &mut executor).expect("complete projected refusal report")
}

fn bounded_row(
    metric: MetricId,
    key: MetricKey,
    value: u64,
    base_limit: u64,
    ratchet_limit: u64,
    breached: bool,
) -> MetricEvidence {
    let (field, kind, parser) = match metric {
        MetricId::FileLines => ("file_loc", "file_loc", ParserId::Utf8FileShapeV1),
        MetricId::FileBytes => ("file_bytes", "file_bytes", ParserId::Utf8FileShapeV1),
        MetricId::RoutineCcn => (
            "routine_ccn",
            "complexity_ccn",
            ParserId::TreeSitterComplexityV1,
        ),
        MetricId::RoutineLength => (
            "routine_length",
            "complexity_length",
            ParserId::TreeSitterComplexityV1,
        ),
        _ => unreachable!("bounded fixture uses a declared latch metric"),
    };
    let transition = if breached { "latched" } else { "clear" };
    let measurement = MetricMeasurement::new(value, ParserEvidence::Native { parser })
        .with_rule(
            RuleBinding::new(field, Some("fold-fixture"), "spice.toml", "path fixture")
                .expect("fixture policy binding"),
        )
        .with_latch(
            LatchBinding::new(
                kind,
                base_limit,
                ratchet_limit,
                0,
                ratchet_limit,
                ratchet_limit,
                false,
                breached,
                breached,
                breached,
                transition,
                None::<String>,
                None::<String>,
            )
            .expect("fixture latch binding"),
        );
    MetricEvidence::new(metric, key, MetricState::measured(measurement))
        .expect("fixture metric evidence")
}

fn magic_row() -> MetricEvidence {
    let measurement = MetricMeasurement::new(
        1,
        ParserEvidence::Native {
            parser: ParserId::CGrammarMagicDeltaV1,
        },
    )
    .with_rule(
        RuleBinding::new(
            "magic.examine_threshold",
            Some("fold-fixture"),
            "spice.toml",
            "path src/magic.rs",
        )
        .expect("magic policy binding"),
    )
    .with_threshold(10);
    MetricEvidence::new(
        MetricId::MagicNumberRegressions,
        MetricKey::path("src/magic.rs").expect("fixture magic path"),
        MetricState::measured(measurement),
    )
    .expect("fixture magic evidence")
}
