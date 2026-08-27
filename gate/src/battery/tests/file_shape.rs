//! File shape and its sibling routine bounds measured against one real tree.
//!
//! These tests run one whole battery over a committed repository. The first
//! pins file-shape selection, document jurisdiction, and the complete bound
//! count; the second pins each routine decision; the third pins what file shape
//! does with a measurement that breaks its bound; the fourth proves a durable
//! latch spends elastic headroom. A fixture that built rows by hand could
//! satisfy any one while the production walk selected nothing.

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
    "spice-rust.toml",
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
/// ratchet reach 1500, and varies that by 5 percent, so no seed the actor jitter
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

/// The complexity arm's result within one complete battery report.
fn complexity_arm(report: &BatteryReport) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::Complexity)
        .expect("the battery ran the complexity arm")
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

/// The one row a named routine carries for one metric.
fn routine_row_for<'report>(
    report: &'report BatteryReport,
    path: &str,
    function: &str,
    metric: MetricId,
) -> &'report MetricEvidence {
    let rows = routine_rows_for(report, path, function);
    let mut matching = rows.into_iter().filter(|row| row.metric() == metric);
    let row = matching
        .next()
        .unwrap_or_else(|| panic!("{path:?}:{function} carries no {} row", metric.as_str()));
    assert!(
        matching.next().is_none(),
        "{path:?}:{function} carries more than one {} row",
        metric.as_str()
    );
    row
}

/// One valid Rust routine spanning exactly `lines` source lines.
fn routine_with_lines(name: &str, lines: usize) -> String {
    assert!(lines >= 2, "a routine needs an opening and closing line");
    let body = "    let _ = 1;\n".repeat(lines - 2);
    format!("pub fn {name}() {{\n{body}}}\n")
}

/// Seed the current file-line latch through the production state transaction.
///
/// Resolving the common Git directory through the same repository model as
/// production keeps the test about the latch input rather than about a
/// hard-coded `.git` layout that linked worktrees reject.
fn seed_file_line_latch(repository: &Path, path: &str) {
    let (_, common_git) = crate::tree::git_directories(repository)
        .expect("the fixture repository publishes its Git state directories");
    let destination = crate::current_state_path(&common_git);
    let mut state = CurrentState::load(&destination).expect("load current gate state");
    state
        .begin_run(&RenameMap::default(), [path], 100.0)
        .expect("begin file-line latch seed");
    state
        .reconcile([LatchObservation::new(
            LatchEntry::new(
                LatchKind::FileLoc,
                StateKey::path(path).expect("fixture path key"),
            )
            .expect("file-line latch key"),
            BoundRule::new("fixture-seed", "file-loc", 10, 15, false, false)
                .expect("file-line seed bound"),
            20,
            ActorInputs::new(path)
                .expect("file-line seed actor inputs")
                .with_active_actor("fixture-actor"),
            0,
        )
        .expect("file-line seed observation")])
        .expect("reconcile file-line latch seed");
    state
        .finish()
        .expect("finish file-line latch seed")
        .accept()
        .expect("persist file-line latch seed");
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

/// A latched path spends its elastic headroom and is held to base.
///
/// Both final reports read the same staged file. The first has an empty durable
/// ledger and admits a value between base and elastic; the second imports that
/// path's `file_loc` latch and rejects the same value against base. Deriving the
/// value from this actor/path pair's first real decision keeps the fixture on
/// the production variation route instead of copying a policy limit or relying
/// on a special-case value.
#[test]
fn a_latched_source_is_held_to_base_while_the_same_unlatched_source_uses_elastic() {
    const PATH: &str = "src/latched.rs";

    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(repository.join(PATH), "// discover this path's headroom\n")
        .expect("provisional source");
    fixture_git(&repository, &["add", PATH]);

    let discovery = battery_report(&repository);
    let (_, discovered) = measured(row_for(&discovery, PATH, MetricId::FileLines), PATH);
    assert!(
        discovered.active_limit() > discovered.base_limit(),
        "this actor/path pair must have elastic headroom: base {}, active {}",
        discovered.base_limit(),
        discovered.active_limit()
    );
    let fitting_lines = discovered
        .base_limit()
        .checked_add(1)
        .expect("the base line limit admits one successor");
    let source = "// held\n".repeat(
        usize::try_from(fitting_lines).expect("the fixture line count fits the address space"),
    );
    fs::write(repository.join(PATH), source).expect("source between base and elastic");
    fixture_git(&repository, &["add", PATH]);

    let unlatched = battery_report(&repository);
    let (unlatched_lines, elastic) = measured(row_for(&unlatched, PATH, MetricId::FileLines), PATH);
    assert_eq!(unlatched_lines, fitting_lines);
    assert_eq!(elastic.base_limit(), discovered.base_limit());
    assert_eq!(elastic.active_limit(), discovered.active_limit());
    assert!(
        elastic.over_base() && !elastic.over_limit(),
        concat!(
            "the unlatched row fits elastic but breaches base: {} ",
            "lines, base {}, active {}"
        ),
        unlatched_lines,
        elastic.base_limit(),
        elastic.active_limit()
    );
    assert_eq!(elastic.transition(), "clear");
    assert_eq!(file_shape_arm(&unlatched).state(), BatteryState::Passed);
    assert!(
        unlatched.context().latches().is_empty(),
        "the comparison report must read an empty durable latch inventory"
    );

    seed_file_line_latch(&repository, PATH);
    let latched = battery_report(&repository);
    let [held_entry] = latched.context().latches() else {
        panic!(
            "the latched report must publish exactly the seeded state entry: {:?}",
            latched.context().latches()
        )
    };
    assert_eq!(held_entry.kind(), "file-loc");
    assert_eq!(held_entry.path(), PATH);
    assert_eq!(held_entry.function(), None);

    let (latched_lines, held) = measured(row_for(&latched, PATH, MetricId::FileLines), PATH);
    assert_eq!(latched_lines, unlatched_lines);
    assert_eq!(held.base_limit(), elastic.base_limit());
    assert_eq!(held.ratchet_limit(), elastic.ratchet_limit());
    assert_eq!(held.active_limit(), held.base_limit());
    assert!(
        held.over_limit() && held.over_base() && !held.ratchet_breach(),
        concat!(
            "the latched row fails through its own spent-headroom decision: ",
            "{} lines, base {}, elastic {}"
        ),
        latched_lines,
        held.base_limit(),
        held.ratchet_limit()
    );
    assert_eq!(held.transition(), "held");
    assert_eq!(
        file_shape_arm(&latched).state(),
        BatteryState::Failed,
        "the arm follows the held row's own latch: {}",
        file_shape_arm(&latched).reason()
    );
}

/// A committed ratchet breach has already spent the next change's headroom.
#[test]
fn a_staged_change_after_a_committed_ratchet_breach_is_held_to_base() {
    const PATH: &str = "src/already-breached.rs";
    const BASE_LIMIT: usize = 1_000;
    const COMMITTED_LINES: usize = 1_600;
    const CANDIDATE_LINES: usize = 1_400;

    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(
        repository.join(PATH),
        "// committed ratchet breach\n".repeat(COMMITTED_LINES),
    )
    .expect("committed source beyond ratchet");
    fixture_git(&repository, &["add", PATH]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "commit file-loc ratchet breach"],
    );

    fs::write(
        repository.join(PATH),
        "// staged improvement\n".repeat(CANDIDATE_LINES),
    )
    .expect("staged source improves the ratchet breach");
    fixture_git(&repository, &["add", PATH]);

    let report = battery_report(&repository);
    let (lines, held) = measured(row_for(&report, PATH, MetricId::FileLines), PATH);
    assert_eq!(lines, CANDIDATE_LINES as u64);
    assert_eq!(held.base_limit(), BASE_LIMIT as u64);
    assert!(lines < held.ratchet_limit());
    assert_eq!(held.active_limit(), BASE_LIMIT as u64);
    assert!(held.over_limit() && held.over_base() && !held.ratchet_breach());
    assert_eq!(held.transition(), "held");
    assert_eq!(file_shape_arm(&report).state(), BatteryState::Failed);
}

/// A committed base breach inside the ratchet retains its flex.
#[test]
fn a_changed_committed_base_breach_inside_ratchet_keeps_headroom() {
    const PATH: &str = "src/flex-debt.rs";
    const BASE_LIMIT: u64 = 1_000;
    const COMMITTED_LINES: usize = 1_200;
    const CANDIDATE_LINES: usize = COMMITTED_LINES + 1;

    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(
        repository.join(PATH),
        "// committed flex\n".repeat(COMMITTED_LINES),
    )
    .expect("committed source inside flex");
    fixture_git(&repository, &["add", PATH]);
    fixture_git(&repository, &["commit", "-q", "-m", "commit flex debt"]);

    fs::write(
        repository.join(PATH),
        "// changed flex\n".repeat(CANDIDATE_LINES),
    )
    .expect("changed source remains inside flex");
    fixture_git(&repository, &["add", PATH]);

    let report = battery_report(&repository);
    let (lines, elastic) = measured(row_for(&report, PATH, MetricId::FileLines), PATH);
    assert_eq!(lines, CANDIDATE_LINES as u64);
    assert_eq!(elastic.base_limit(), BASE_LIMIT);
    assert!(elastic.active_limit() > BASE_LIMIT);
    assert!(lines < elastic.active_limit());
    assert!(elastic.over_base() && !elastic.over_limit());
    assert_eq!(elastic.transition(), "clear");
    assert_eq!(file_shape_arm(&report).state(), BatteryState::Passed);
}

/// An edited over-base routine inside its ratchet retains flex.
#[test]
fn a_changed_routine_inside_the_ratchet_keeps_headroom() {
    const PATH: &str = "src/routine-debt.rs";
    const FUNCTION: &str = "authored";
    const BASE: u64 = 80;

    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(repository.join(PATH), routine_with_lines(FUNCTION, 100))
        .expect("committed routine above the length base");
    fixture_git(&repository, &["add", PATH]);
    fixture_git(&repository, &["commit", "-q", "-m", "commit routine debt"]);

    let unchanged = battery_report(&repository);
    let row = routine_row_for(&unchanged, PATH, FUNCTION, MetricId::RoutineLength);
    let (_, elastic) = measured(row, PATH);
    assert_eq!(
        (elastic.base_limit(), elastic.configured_ratchet_limit()),
        (BASE, 120)
    );
    assert!(elastic.active_limit() > BASE && !elastic.over_limit());
    assert_eq!(elastic.transition(), "clear");

    fs::write(repository.join(PATH), routine_with_lines(FUNCTION, 90))
        .expect("improve the routine without reaching the length base");
    fixture_git(&repository, &["add", PATH]);
    let changed = battery_report(&repository);
    let row = routine_row_for(&changed, PATH, FUNCTION, MetricId::RoutineLength);
    let (length, elastic) = measured(row, PATH);
    assert!(elastic.active_limit() > BASE);
    assert!(length > BASE && length < elastic.active_limit());
    assert!(!elastic.over_limit() && !elastic.ratchet_breach());
    assert_eq!(elastic.transition(), "clear");
    assert_eq!(complexity_arm(&changed).state(), BatteryState::Passed);
}
