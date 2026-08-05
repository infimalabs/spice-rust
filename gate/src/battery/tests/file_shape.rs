//! File shape and its sibling routine bounds measured against one real tree.
//!
//! These tests run one whole battery over a committed repository. The first
//! pins file-shape selection, document jurisdiction, and the complete bound
//! count; the second pins each routine decision; the third pins what file shape
//! does with a measurement that breaks its bound. A fixture that built rows by
//! hand could satisfy any one while the production walk selected nothing.

use super::*;
use crate::metrics::{
    EvidenceState as MetricState, LatchBinding, MetricEvidence, MetricId, MetricKey,
};

/// Every committed source the fixture repository expects the arm to measure.
///
/// Spelled out rather than derived from the tree, because a list the subject
/// produces cannot notice a path the subject stopped selecting. The seven cover
/// three suffixes across the root and three directories, so a selection
/// narrowed to one language or one directory fails here rather than passing
/// over what is left.
const SELECTED_SOURCES: [&str; 7] = [
    "app/__init__.py",
    "app/main.py",
    "baseline.toml",
    "pyproject.toml",
    "spice.toml",
    "src/lib.rs",
    "tests/test_main.py",
];

/// Every routine the same fixture's complexity arm measures, with both values.
///
/// Each lives in a different path, so the path carried by [`BoundDecision`]
/// plus the metric identifies one exact routine decision. The function name is
/// still asserted against the arm row, closing the identity back to the parsed
/// routine rather than inferring it from the context count.
const SELECTED_ROUTINES: [(&str, &str, u64, u64); 3] = [
    ("app/main.py", "entry", 1, 2),
    ("src/lib.rs", "fixture", 1, 1),
    ("tests/test_main.py", "test_entry", 1, 2),
];

/// A source long enough that no elastic headroom can admit it.
///
/// The tracked policy this fixture copies caps `file_loc` at 1000 lines, lets a
/// ratchet reach 1500, and varies that by 7 percent, so no seed the actor jitter
/// can produce comes near this. A length that only sometimes breached would make
/// the verdict a property of which actor name the harness happened to use.
const OVERLONG_LINES: usize = 4000;

/// The file-shape arm's result within one complete battery report.
fn file_shape_arm(report: &BatteryReport) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::FileShape)
        .expect("the battery ran the file-shape arm")
}

/// Every row the file-shape arm carries for one selected path.
fn rows_for<'report>(report: &'report BatteryReport, path: &str) -> Vec<&'report MetricEvidence> {
    let ArmEvidence::Metrics { report } = file_shape_arm(report).evidence() else {
        panic!("the file-shape arm projects structural metric evidence")
    };
    let key = MetricKey::path(path).expect("a selected path key");
    report
        .metrics()
        .iter()
        .filter(|metric| metric.key() == &key)
        .collect()
}

/// Both complexity rows the arm carries for one exact parsed routine.
fn routine_rows_for<'report>(
    report: &'report BatteryReport,
    path: &str,
    function: &str,
) -> Vec<&'report MetricEvidence> {
    let result = report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::Complexity)
        .expect("the battery ran the complexity arm");
    let ArmEvidence::Metrics { report } = result.evidence() else {
        panic!("the complexity arm projects structural metric evidence")
    };
    let key = MetricKey::function(path, function).expect("a fixture routine key");
    report
        .metrics()
        .iter()
        .filter(|metric| metric.key() == &key)
        .collect()
}

/// One row's measured value and the latch it was decided under.
fn measured<'row>(row: &'row MetricEvidence, path: &str) -> (u64, &'row LatchBinding) {
    let MetricState::Measured { measurement } = row.state() else {
        panic!(
            "{path:?} was expected to measure {}, but carried {:?}",
            row.metric().as_str(),
            row.state()
        )
    };
    let latch = measurement.latch().unwrap_or_else(|| {
        panic!(
            "{path:?} measured {} without the bound it was judged against",
            row.metric().as_str()
        )
    });
    (measurement.value(), latch)
}

/// The one row a path carries for one metric.
fn row_for<'report>(
    report: &'report BatteryReport,
    path: &str,
    metric: MetricId,
) -> &'report MetricEvidence {
    let rows = rows_for(report, path);
    let mut matching = rows.iter().filter(|row| row.metric() == metric);
    let row = matching
        .next()
        .unwrap_or_else(|| panic!("{path:?} carries no {} row", metric.as_str()));
    assert!(
        matching.next().is_none(),
        "{path:?} carries more than one {} row",
        metric.as_str()
    );
    row
}

/// Selected sources and routines publish their bounds; documents keep their own.
///
/// The values are compared against the bytes on disk rather than against each
/// other, so an arm that measured the wrong file — or measured one file twice
/// under two keys — cannot agree with itself into a pass. Every path is asserted
/// to carry exactly two rows, which is the pairing the arm's declaration
/// requires expressed per path rather than as a set the whole arm satisfies:
/// two paths each missing the other's metric would produce equal keysets.
///
/// `app/notes.md` closes the jurisdiction. Its suffix is in the file-shape
/// source class, so an arm selecting on suffix alone would measure it, and it
/// would then answer to a line limit here and a character limit on the
/// repository-document arm at once. The absence is asserted beside that arm's
/// own measurement of the same file, because an absence on its own would also
/// be produced by an arm that had stopped reaching Markdown entirely.
#[test]
fn each_selected_source_measures_both_shapes_and_documents_stay_with_their_own_bound() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);

    let report = battery_report(&repository);
    assert_eq!(
        report.context().bounds().len(),
        1 + SELECTED_SOURCES.len() * 2 + SELECTED_ROUTINES.len() * 2,
        "one document bound plus every file-shape and routine decision are published"
    );

    for path in SELECTED_SOURCES {
        let bytes = fs::read(repository.join(path)).expect("the fixture committed this source");
        let decisions = report
            .context()
            .bounds()
            .iter()
            .filter(|decision| {
                decision.actor_inputs().candidate_path() == path
                    && matches!(
                        decision.metric(),
                        metric if metric == MetricId::FileLines.as_str()
                            || metric == MetricId::FileBytes.as_str()
                    )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            decisions.len(),
            2,
            "{path:?} must publish both structural decisions, but reported {decisions:?}"
        );
        assert_eq!(
            decisions
                .iter()
                .map(|decision| decision.metric())
                .collect::<Vec<_>>(),
            [MetricId::FileLines.as_str(), MetricId::FileBytes.as_str()],
            "{path:?} published decisions for the wrong structural metrics"
        );
        let rows = rows_for(&report, path);
        assert_eq!(
            rows.len(),
            2,
            "{path:?} carries one row per declared shape metric, but reported {:?}",
            rows.iter().map(|row| row.metric()).collect::<Vec<_>>()
        );
        let (lines, _) = measured(row_for(&report, path, MetricId::FileLines), path);
        assert_eq!(
            lines,
            u64::try_from(bytes.iter().filter(|byte| **byte == b'\n').count())
                .expect("a fixture source shorter than the address space"),
            "{path:?} measured a line count its committed bytes do not hold"
        );
        let (measured_bytes, _) = measured(row_for(&report, path, MetricId::FileBytes), path);
        assert_eq!(
            measured_bytes,
            u64::try_from(bytes.len()).expect("a fixture source shorter than the address space"),
            "{path:?} measured a byte count its committed bytes do not hold"
        );
        assert_eq!(
            decisions
                .iter()
                .map(|decision| decision.measured())
                .collect::<Vec<_>>(),
            [lines, measured_bytes],
            "{path:?} reported reconstructed bounds instead of the decisions carried by its rows"
        );
    }

    assert!(
        rows_for(&report, "app/notes.md").is_empty(),
        "a repository document answers to its character bound, not to a line one"
    );
    let documents = report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::RepoDocs)
        .expect("the battery ran the repository-document arm");
    let ArmEvidence::Core {
        report: document_report,
    } = documents.evidence()
    else {
        panic!("the repository-document arm projects core evidence")
    };
    let Some(CoreEvidence::RepoDocs(documents)) = document_report.evidence() else {
        panic!(
            "the repository-document arm measured the documents it selected, but carried {:?}",
            document_report.evidence()
        )
    };
    assert!(
        documents
            .measurements()
            .iter()
            .any(|measurement| measurement.path() == "app/notes.md"),
        "the Markdown the file-shape arm skipped is measured by the arm that owns it"
    );
    let document_bounds = report
        .context()
        .bounds()
        .iter()
        .filter(|bound| bound.actor_inputs().candidate_path() == "app/notes.md")
        .collect::<Vec<_>>();
    assert_eq!(
        document_bounds.len(),
        1,
        "the context must retain the document's core decision alongside the structural ones"
    );
    assert_eq!(
        document_bounds[0].metric(),
        MetricId::RepositoryDocumentCharacters.as_str()
    );
}

/// Each parsed routine publishes the exact decisions its measured rows used.
///
/// The function key is asserted in the arm rows while the path, metric, and
/// value are asserted in the run context. Binding both views keeps the total of
/// 21 from hiding a dropped routine behind a duplicated file-shape decision.
#[test]
fn each_measured_routine_publishes_both_complexity_decisions() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let report = battery_report(&repository);

    for (path, function, expected_ccn, expected_length) in SELECTED_ROUTINES {
        let rows = routine_rows_for(&report, path, function);
        assert_eq!(rows.len(), 2, "{path:?} routine {function:?} row count");
        let decisions = report
            .context()
            .bounds()
            .iter()
            .filter(|decision| {
                decision.actor_inputs().candidate_path() == path
                    && matches!(
                        decision.metric(),
                        metric if metric == MetricId::RoutineCcn.as_str()
                            || metric == MetricId::RoutineLength.as_str()
                    )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            decisions.len(),
            2,
            "{path:?} routine {function:?} decisions"
        );
        let expected = [
            (MetricId::RoutineCcn.as_str(), expected_ccn),
            (MetricId::RoutineLength.as_str(), expected_length),
        ];
        let published = decisions
            .iter()
            .map(|decision| (decision.metric(), decision.measured()))
            .collect::<Vec<_>>();
        assert_eq!(
            published, expected,
            "{path:?} routine {function:?} identity"
        );
        let measured = rows
            .iter()
            .map(|row| (row.metric().as_str(), measured(row, path).0))
            .collect::<Vec<_>>();
        assert_eq!(published, measured, "{path:?} routine {function:?} bounds");
    }
}

/// A file past its elastic limit fails the arm through the bound on its own row.
///
/// The row is what carries the verdict: it measures, and the latch it measured
/// under reports the breach. Asserting the arm state alone would pass for an arm
/// that failed for any reason at all, and asserting the row alone would pass for
/// one that recorded the breach and admitted the tree anyway.
///
/// The paths that fit still measure in the same arm, which is what separates a
/// bound decided per path from an arm that gave up at the first breach. Their
/// latches are asserted to be seeded from the acting actor, since an elastic
/// limit that ignored provenance would hand every path the same headroom and the
/// breach above would stop being a property of this file under this actor.
#[test]
fn a_source_over_its_elastic_limit_fails_through_the_bound_on_its_own_row() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let overlong = "pub fn spacer() {}\n".repeat(OVERLONG_LINES);
    fs::write(repository.join("src/overlong.rs"), &overlong).expect("fixture overlong source");
    // Staged rather than merely written, because the selection is the index.
    fixture_git(&repository, &["add", "src/overlong.rs"]);

    let report = battery_report(&repository);

    let (lines, latch) = measured(
        row_for(&report, "src/overlong.rs", MetricId::FileLines),
        "src/overlong.rs",
    );
    assert_eq!(
        lines,
        u64::try_from(OVERLONG_LINES).expect("a fixture source shorter than the address space"),
        "the breaching row measures the file rather than reporting the breach alone"
    );
    assert!(
        latch.over_limit(),
        "the row's own latch reports the breach: {lines} against an active limit of {}",
        latch.active_limit()
    );
    assert!(
        lines > latch.active_limit() && latch.active_limit() >= latch.base_limit(),
        "the limit the row was judged against is the elastic one it exceeded: \
         {lines} lines, base {}, active {}",
        latch.base_limit(),
        latch.active_limit()
    );
    assert_eq!(
        file_shape_arm(&report).state(),
        BatteryState::Failed,
        "the arm fails on the bound its own row reports: {}",
        file_shape_arm(&report).reason()
    );

    let (_, fitting) = measured(
        row_for(&report, "src/lib.rs", MetricId::FileLines),
        "src/lib.rs",
    );
    assert!(
        !fitting.over_limit(),
        "the paths that fit are measured beside the one that does not"
    );
    assert_eq!(
        fitting.actor(),
        Some(FIXTURE_BATTERY_ACTOR),
        "the elastic limit is seeded from the actor this battery was captured under"
    );
    assert_eq!(
        fitting.actor_source(),
        Some("active-actor"),
        "and that actor reached the bound as the acting one, not as a candidate"
    );
}
