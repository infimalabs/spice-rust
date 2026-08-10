//! The refusal fold is bounded human guidance, never report authority.

use super::*;
use crate::battery::refusal_fold::REFUSAL_FOLD_SITE_LIMIT;
use crate::metrics::{
    EvidenceState as MetricState, LatchBinding, MetricEvidence, MetricFinding, MetricId, MetricKey,
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
    let finding_arms = refusing
        .iter()
        .filter(|result| result.state() == BatteryState::Failed)
        .count();
    let summary_start = text
        .rfind("battery refusal summary:")
        .expect("refusal fold carries a terminal summary");
    let metric_findings = text[..summary_start].matches("finding gate=").count();
    let summary = format!(
        concat!(
            "battery refusal summary: total_arms={} admitted_arms={} ",
            "finding_arms={} cannot_run_arms={} metric_findings={} ",
            "omitted_arms=0 omitted_findings=0\n"
        ),
        report.results().len(),
        report.results().len() - refusing.len(),
        finding_arms,
        refusing.len() - finding_arms,
        metric_findings,
    );
    assert!(
        text.ends_with(&summary),
        "the board is followed by its score: {text}"
    );
    assert!(
        text.starts_with("arm position="),
        "refusal evidence must lead its summary: {text}"
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
fn gate_findings_project_every_non_admitting_metric_key_once() {
    // GOV-51 / CASE 256: instruments report typed findings exactly once per
    // non-admitting evidence key; dispatch, not the instrument, decides filing.
    let report = metric_breach_report();
    let findings = report.findings();

    assert_eq!(findings.len(), 3);
    let identities = findings
        .iter()
        .map(|finding| finding.identity().clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identities.len(), findings.len());
    assert!(findings.iter().any(|finding| {
        finding.locus().contains("metric=file.lines")
            && finding.description().contains("Value: 201")
            && finding.acceptance()[0].contains("file-shape")
    }));
    assert!(findings.iter().any(|finding| {
        finding.locus().contains("metric=routine.ccn")
            && finding.locus().contains("function=\"complicated\"")
    }));
    assert!(findings.iter().any(|finding| {
        finding.locus().contains("metric=magic-number.regressions")
            && finding.description().contains("Threshold: 10")
    }));
}

/// The fold names where inside the refused file the breach actually is.
///
/// A metric key names a file and a value says how many times it offends; before
/// the measurement carried findings the fold stopped exactly there, so the
/// reader of a refusal was told `retired_names.rs` has one phantom citation and
/// left to find it by reading. Both halves are asserted: an absence has no
/// correct spelling to offer and must not invent one, while a case mismatch
/// knows the tracked spelling and is useless without it.
#[test]
fn a_metric_fold_names_the_site_inside_the_file_it_refused() {
    let report = markdown_breach_report(vec![
        (
            MetricId::RustDocMarkdownTargetAbsences,
            "gate/tests/retired_names.rs",
            vec![finding("gate/tests/retired_names.rs", 3, "NAMES.md", None)],
        ),
        (
            MetricId::MarkdownLinkCaseMismatches,
            "docs/guide.md",
            vec![finding(
                "docs/guide.md",
                1,
                "Target.md",
                Some("docs/target.md"),
            )],
        ),
    ]);
    let fold = report.refusal_fold();
    let text = fold.as_str();

    assert!(
        text.contains("  site path=\"gate/tests/retired_names.rs\" line=3 target=\"NAMES.md\"\n"),
        "the fold names the citing line and the unresolved target: {text}"
    );
    assert!(
        text.contains(concat!(
            "  site path=\"docs/guide.md\" line=1 target=\"Target.md\" ",
            "expected=\"docs/target.md\"\n"
        )),
        "a miscased link names the spelling written and the spelling tracked: {text}"
    );
    assert!(
        !text.contains("sites-undisplayed"),
        "a complete listing must not claim it was shortened: {text}"
    );
    assert_eq!(fold.omitted_findings(), 0);
}

/// One pathological file cannot spend the fold on itself, and says so.
///
/// The count of undisplayed sites is the whole point of the bound being visible:
/// a reader who sees three sites and no counter reasonably concludes there were
/// three, fixes them, and is refused again by the ones never shown.
#[test]
fn a_fold_shortens_a_long_site_listing_and_states_how_many_it_withheld() {
    let planted = REFUSAL_FOLD_SITE_LIMIT + 2;
    let findings: Vec<_> = (0..planted)
        .map(|index| {
            finding(
                "docs/many.md",
                index as u64 + 1,
                &format!("Note{index}.md"),
                Some(&format!("docs/note{index}.md")),
            )
        })
        .collect();
    let report = markdown_breach_report(vec![(
        MetricId::MarkdownLinkCaseMismatches,
        "docs/many.md",
        findings,
    )]);
    let text = report.refusal_fold().as_str().to_owned();

    assert_eq!(
        text.matches("  site path=").count(),
        REFUSAL_FOLD_SITE_LIMIT,
        "the fold renders exactly the bounded listing: {text}"
    );
    assert!(
        text.contains(&format!("value={planted}")),
        "the count stays whole: shortening the listing is not shortening the breach: {text}"
    );
    assert!(
        text.contains(&format!(
            "  sites-undisplayed={}\n",
            planted - REFUSAL_FOLD_SITE_LIMIT
        )),
        "the fold states how many sites it withheld: {text}"
    );
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

fn finding(path: &str, line: u64, target: &str, expected: Option<&str>) -> MetricFinding {
    MetricFinding {
        path: path.to_owned(),
        line,
        target: target.to_owned(),
        expected: expected.map(str::to_owned),
    }
}

/// A Markdown arm refusing on zero tolerance, with the sites it refused over.
fn markdown_breach_report(rows: Vec<(MetricId, &str, Vec<MetricFinding>)>) -> BatteryReport {
    let fixture = Fixture::new();
    let mut evidence = fixtures::passing(&fixture.source);
    let present: Vec<_> = rows.iter().map(|(metric, _, _)| *metric).collect();
    let mut metrics: Vec<_> = rows
        .into_iter()
        .map(|(metric, path, findings)| {
            let measurement = MetricMeasurement::new(
                findings.len() as u64,
                ParserEvidence::Native {
                    parser: ParserId::MarkdownTrackedTargetsV2,
                },
            )
            .with_findings(findings);
            MetricEvidence::new(
                metric,
                MetricKey::path(path).expect("markdown fixture path key"),
                MetricState::measured(measurement),
            )
            .expect("markdown fixture metric evidence")
        })
        .collect();
    // The arm is complete or it is not an arm: a Markdown metric this fixture
    // does not breach still has to say why it measured nothing.
    for metric in [
        MetricId::MarkdownLinkCaseMismatches,
        MetricId::RustDocMarkdownTargetAbsences,
    ] {
        if !present.contains(&metric) {
            metrics.push(
                MetricEvidence::new(
                    metric,
                    MetricKey::Arm,
                    MetricState::NotApplicable {
                        reason: "fixture plants no breach for this metric".to_owned(),
                    },
                )
                .expect("not-applicable markdown metric evidence"),
            );
        }
    }
    replace_metric_arm(&mut evidence, GateId::MarkdownLinks, metrics);
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
            RuleBinding::new(
                field,
                Some("fold-fixture"),
                "spice-rust.toml",
                "path fixture",
            )
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
            "spice-rust.toml",
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
