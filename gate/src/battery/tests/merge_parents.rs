//! A merge charged for what it wrote, and not for what it only carried across.
//!
//! Both cases run one whole battery over the same repository shape: a short
//! Rust source is committed on the base both branches share, the second parent
//! grows it past every file-loc limit and adds a comparison-adjacent literal to
//! it, and `main` is merged with that. The two cases differ in one respect only
//! — whether the merging lane then touched that file — because that is the
//! entire distinction being claimed. A gate that read the first parent alone
//! reports the same two failures either way, since `HEAD` still carries the
//! short literal-free version the merge is not bringing forward.
//!
//! The passing case alone would also pass for a gate that simply stopped
//! measuring during a merge, which is why the refusing case appends one line to
//! that same inherited file and asserts both arms come back. Between them the
//! exemption is proved to be keyed on the file's bytes rather than on the
//! presence of `MERGE_HEAD`.

use super::*;
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricId, MetricKey};

/// The source the second parent commits and the merge inherits verbatim.
const INHERITED: &str = "src/inherited.rs";

/// The short source the merge itself writes, standing for its resolutions.
const RESOLVED: &str = "src/resolved.rs";

/// Lines that put `INHERITED` past `file_loc` however its headroom varies.
///
/// The tracked policy this fixture copies caps `file_loc` at 1000 lines and
/// varies the elastic limit per path and actor, so a file near the cap could
/// land either side of it. Four thousand cannot, which keeps the refusing case
/// below about the merge's own line rather than about a jitter draw.
const INHERITED_LINES: usize = 4000;

/// The comparison-adjacent literal the second parent commits into `INHERITED`.
const INHERITED_LITERAL: u64 = 4242;

/// The comparison-adjacent literal the merging lane adds on top of it.
///
/// Spelled apart from `INHERITED_LITERAL` so the refusing case cannot be
/// satisfied by the inherited one being counted twice.
const MERGED_LITERAL: u64 = 31337;

/// One Rust source of the requested length holding one compared literal.
///
/// The literal is interpolated rather than written out because this file is
/// itself a `.rs` source in this repository's declared magic class: a number
/// spelled next to a comparison operator here would be a new magic number in
/// *this* tree, introduced by the test for the arm that reports them.
fn compared_source(literal: u64, lines: usize) -> String {
    let mut source = format!("pub fn admits(value: u64) -> bool {{\n    value >= {literal}\n}}\n");
    while source.lines().count() < lines {
        source.push_str("pub fn spacer() {}\n");
    }
    source
}

/// One arm's result within a complete battery report.
fn arm(report: &BatteryReport, id: GateId) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == id)
        .unwrap_or_else(|| panic!("the battery ran the {id:?} arm"))
}

/// One path's row for one metric within one arm.
fn row<'report>(
    report: &'report BatteryReport,
    id: GateId,
    path: &str,
    metric: MetricId,
) -> &'report MetricEvidence {
    let ArmEvidence::Metrics { report } = arm(report, id).evidence() else {
        panic!("the {id:?} arm projects structural metric evidence")
    };
    let key = MetricKey::path(path).expect("a selected path key");
    report
        .metrics()
        .iter()
        .find(|candidate| candidate.key() == &key && candidate.metric() == metric)
        .unwrap_or_else(|| {
            panic!(
                "the {id:?} arm carries a {} row for {path:?}, but its keys are {:?}",
                metric.as_str(),
                report
                    .metrics()
                    .iter()
                    .map(MetricEvidence::key)
                    .collect::<Vec<_>>()
            )
        })
}

/// The value one row measured, or a panic naming the state it carried instead.
fn measured(row: &MetricEvidence, path: &str) -> u64 {
    let MetricState::Measured { measurement } = row.state() else {
        panic!(
            "{path:?} was expected to measure {}, but carried {:?}",
            row.metric().as_str(),
            row.state()
        )
    };
    measurement.value()
}

/// Commit `INHERITED` on a side branch and merge it into `main`, uncommitted.
///
/// The merge is left with `MERGE_HEAD` in place and the merged content staged,
/// which is exactly the state `spice dev pre-commit` observes when Git invokes
/// it for the merge commit.
fn merged_repository(root: &Path) -> PathBuf {
    let repository = production_repository(root);
    // Taken out of the index before either parent exists, so neither branch and
    // neither case below carries it. Left committed, the magic arm folds to its
    // refusal to read the class and stops answering the question asked here.
    for path in PREDECESSOR_CLASS_SOURCES {
        fixture_git(&repository, &["rm", "-q", "--cached", path]);
    }
    // Committed short and literal-free on the base both branches share, so the
    // side branch grows a file `HEAD` already carries. A file merely absent from
    // `HEAD` would reach the magic arm as having no baseline at all, which is
    // inapplicability rather than the regression this is about.
    fs::write(repository.join(INHERITED), "pub fn inherited() {}\n").expect("fixture base source");
    fixture_git(&repository, &["add", INHERITED]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "drop the unreadable class"],
    );

    fixture_git(&repository, &["checkout", "-q", "-b", "side"]);
    fs::write(
        repository.join(INHERITED),
        compared_source(INHERITED_LITERAL, INHERITED_LINES),
    )
    .expect("fixture inherited source");
    fixture_git(&repository, &["add", INHERITED]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "side authors the file"],
    );

    fixture_git(&repository, &["checkout", "-q", "main"]);
    fs::write(repository.join("src/lib.rs"), "pub fn fixture_moved() {}\n")
        .expect("fixture main-side edit");
    fixture_git(&repository, &["add", "src/lib.rs"]);
    fixture_git(&repository, &["commit", "-q", "-m", "main moves elsewhere"]);

    fixture_git(&repository, &["merge", "--no-commit", "--no-ff", "side"]);
    assert!(
        repository.join(".git/MERGE_HEAD").exists(),
        "the fixture merge must leave MERGE_HEAD in place"
    );
    // The merge writes one short source of its own, the way a real merge carries
    // its resolutions. Without it every selected path in this tree is some
    // parent's, the arm has nothing left to measure, and it would report that it
    // never applied — which passes the case below for the wrong reason and
    // cannot tell a per-file exemption from a merge-wide amnesty.
    fs::write(repository.join(RESOLVED), "pub fn resolved() {}\n").expect("the merge's resolution");
    fixture_git(&repository, &["add", RESOLVED]);
    repository
}

/// A merge is charged for neither the shape nor the literals it only carried.
///
/// The second parent's file is past `file_loc` and holds a literal `HEAD` has
/// never seen, so a gate comparing the staged tree against the first parent
/// alone fails both arms here. Both are asserted, and each is asserted at its
/// own row rather than only at the arm, so an arm that passed for some unrelated
/// reason cannot stand in for the exemption.
///
/// The file's measured length is *not* asserted, because the claim is that this
/// run does not measure it at all: it takes no bound decision, so it can neither
/// breach an elastic limit nor be handed the flex slice that a breach latches.
#[test]
fn a_merge_is_not_charged_for_a_file_its_second_parent_already_committed() {
    let fixture = Fixture::new();
    let repository = merged_repository(&fixture.root);

    let report = battery_report(&repository);

    let shape = row(&report, GateId::FileShape, INHERITED, MetricId::FileLines);
    assert!(
        matches!(shape.state(), MetricState::NotApplicable { .. }),
        "the inherited file takes no file-loc decision, but carried {:?}",
        shape.state()
    );
    assert!(
        measured(
            row(&report, GateId::FileShape, RESOLVED, MetricId::FileLines),
            RESOLVED,
        ) > 0,
        "the file the merge did write is measured beside the one it inherited"
    );
    assert_eq!(
        arm(&report, GateId::FileShape).state(),
        BatteryState::Passed,
        "the file-shape arm admits a merge that authored no over-limit shape: {}",
        arm(&report, GateId::FileShape).reason()
    );
    assert_eq!(
        measured(
            row(
                &report,
                GateId::MagicNumbers,
                INHERITED,
                MetricId::MagicNumberRegressions,
            ),
            INHERITED,
        ),
        0,
        "a literal committed by the second parent is not introduced by the merge"
    );
    assert_eq!(
        arm(&report, GateId::MagicNumbers).state(),
        BatteryState::Passed,
        "the magic-number arm admits a merge that introduced no literal: {}",
        arm(&report, GateId::MagicNumbers).reason()
    );
}

/// The same merge is charged in full once it writes to that same file.
///
/// One line is appended and one literal added to the file the case above let
/// through. Both arms refuse, which is what separates a per-file authorship
/// question from a merge-wide amnesty: the exemption is keyed on the bytes being
/// a parent's, and these bytes are no parent's.
///
/// An edited file is then measured against the nearest parent that carries it,
/// so the count below is both literals rather than the one this lane typed. That
/// is the stated rule and not an oversight: the guarantee a merge is given is
/// for the files it left alone, and a lane that edits an inherited file is
/// presenting the result as its own. Narrowing it to what the merge typed would
/// mean asking each parent separately, which is a different contract than the
/// one this fixture pins.
#[test]
fn a_merge_is_charged_for_the_line_it_adds_to_an_inherited_file() {
    let fixture = Fixture::new();
    let repository = merged_repository(&fixture.root);
    let mut resolved = compared_source(INHERITED_LITERAL, INHERITED_LINES);
    resolved.push_str(&format!(
        "pub fn merged(value: u64) -> bool {{\n    value >= {MERGED_LITERAL}\n}}\n"
    ));
    fs::write(repository.join(INHERITED), &resolved).expect("the merge's own edit");
    fixture_git(&repository, &["add", INHERITED]);

    let report = battery_report(&repository);

    let lines = measured(
        row(&report, GateId::FileShape, INHERITED, MetricId::FileLines),
        INHERITED,
    );
    assert_eq!(
        lines,
        u64::try_from(resolved.lines().count()).expect("a fixture source fits u64"),
        "a file the merge edited is measured rather than passed over"
    );
    assert_eq!(
        arm(&report, GateId::FileShape).state(),
        BatteryState::Failed,
        "the file-shape arm still refuses the shape this merge now owns"
    );
    assert_eq!(
        measured(
            row(
                &report,
                GateId::MagicNumbers,
                INHERITED,
                MetricId::MagicNumberRegressions,
            ),
            INHERITED,
        ),
        [INHERITED_LITERAL, MERGED_LITERAL].len() as u64,
        "a file the merge edited is measured against the nearest parent carrying \
         it, which is HEAD, where neither literal is present yet"
    );
    assert_eq!(
        arm(&report, GateId::MagicNumbers).state(),
        BatteryState::Failed,
        "the magic-number arm still refuses the literal this merge introduced"
    );
}
