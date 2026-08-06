//! The magic-number arm measured against real committed bytes.
//!
//! Every assertion here rests on the same repository, because the cases only
//! mean anything beside each other: one file that was committed with literals
//! and left alone, one that was committed with literals and given one more, one
//! that has no committed bytes at all, and one in a source class this build
//! cannot read. A scan handed no baseline reports the same nonzero count for the
//! first three, so a test that showed only the third — or only the first — could
//! not tell a delta from a plain scan. The fourth is what neither of the others
//! can see: a row where both a missing baseline and an unreadable class are true
//! at once, which is answered by class and not by count.

use super::*;
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricKey, MetricMeasurement};

/// The literals the fixture's committed sources already hold.
///
/// Each clears the packaged `magic.examine_threshold` on its own, so the scanner
/// has something to find in every one of these files before any change is made
/// to them. A file whose literals were all below that bar would report zero
/// regressions whether or not its baseline was ever read — which is why the
/// threshold is read back from the arm by `examine_threshold` rather than
/// restated here, where it could quietly stop being true of these numbers.
const COMMITTED_LITERALS: [u64; 3] = [4242, 7777, 9999];

/// The one literal added to an already-committed source.
const INTRODUCED_LITERAL: u64 = 31337;

/// One Rust source that compares a value against each literal it is given.
///
/// The literals are interpolated rather than written out, because this file is
/// itself a `.rs` source in this repository's declared magic class: a number
/// spelled next to a comparison operator here would be a new magic number in
/// *this* tree, introduced by the test for the arm that reports them.
fn comparing_source(literals: &[u64]) -> String {
    literals
        .iter()
        .enumerate()
        .map(|(index, literal)| {
            format!("pub fn admits_{index}(value: u64) -> bool {{\n    value >= {literal}\n}}\n")
        })
        .collect()
}

/// The one battery result the magic-number arm published.
fn magic_arm(report: &BatteryReport) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::MagicNumbers)
        .expect("the battery ran the magic-number arm")
}

/// The single state the magic-number arm folded its rows into.
fn arm_state(report: &BatteryReport) -> BatteryState {
    magic_arm(report).state()
}

/// One committed file's own row in the acquired magic-number arm.
fn magic_row<'report>(report: &'report BatteryReport, path: &str) -> &'report MetricEvidence {
    let ArmEvidence::Metrics { report } = magic_arm(report).evidence() else {
        panic!("the magic-number arm projects structural metric evidence")
    };
    report
        .metrics()
        .iter()
        .find(|metric| metric.key() == &MetricKey::path(path).expect("a selected path key"))
        .unwrap_or_else(|| {
            panic!(
                "the arm carries one row per selected magic source, but {path:?} \
                 is absent from {:?}",
                report
                    .metrics()
                    .iter()
                    .map(MetricEvidence::key)
                    .collect::<Vec<_>>()
            )
        })
}

/// The measurement one row carries, or a panic naming the state it carried.
fn measurement<'row>(evidence: &'row MetricEvidence, path: &str) -> &'row MetricMeasurement {
    match evidence.state() {
        MetricState::Measured { measurement } => measurement,
        state => panic!("{path:?} was expected to measure, but carried {state:?}"),
    }
}

/// The value one row measured.
fn measured(evidence: &MetricEvidence, path: &str) -> u64 {
    measurement(evidence, path).value()
}

/// The `magic.examine_threshold` the arm resolved for one measured path.
///
/// Read back off the published row rather than written here, so a control scan
/// examines the literals the arm examined. A threshold spelled in this file
/// would agree with the packaged one only until the package moved: raised above
/// the literals committed below, the arm would measure zero for want of anything
/// to find, while a control still scanning at the old number would find all
/// three and report a delta the arm never took.
fn examine_threshold(evidence: &MetricEvidence, path: &str) -> u64 {
    measurement(evidence, path)
        .threshold()
        .unwrap_or_else(|| panic!("{path:?} records the threshold its scan examined at"))
}

/// A committed file's existing literals are not regressions; a new one is.
///
/// This is the case a missing baseline gets wrong, and it is asserted from both
/// sides of the same file. `src/kept.rs` holds three comparison-adjacent
/// literals and has not changed since it was committed, so its delta is zero.
/// The same three literals are then proven to be findings in their own right by
/// scanning that file with no baseline, which is exactly what an arm that never
/// reached the committed bytes would do: the zero above is a delta, not an empty
/// scan or an unread file.
///
/// `src/grown.rs` closes the other side. It was committed with the same three
/// literals and then given a fourth, so a delta that read the baseline reports
/// exactly one regression — not four, and not none. Without that row a baseline
/// that returned the *current* bytes for every path would pass this test just as
/// well as a correct one, since it would also report zero for the unchanged file.
#[test]
fn a_committed_file_regresses_only_by_what_was_added_to_it() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let kept = comparing_source(&COMMITTED_LITERALS);
    fs::write(repository.join("src/kept.rs"), &kept).expect("fixture unchanged source");
    fs::write(repository.join("src/grown.rs"), &kept).expect("fixture grown source");
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "literals"]);

    let mut grown = COMMITTED_LITERALS.to_vec();
    grown.push(INTRODUCED_LITERAL);
    fs::write(repository.join("src/grown.rs"), comparing_source(&grown))
        .expect("one added literal");
    // Staged rather than merely written, because the selection is the index.
    fixture_git(&repository, &["add", "src/grown.rs"]);

    let report = battery_report(&repository);

    let kept_row = magic_row(&report, "src/kept.rs");
    assert_eq!(
        measured(kept_row, "src/kept.rs"),
        0,
        "an unchanged file's committed literals are not regressions"
    );
    assert_eq!(
        crate::metrics::scan_magic_regressions(
            "src/kept.rs",
            &kept,
            None,
            examine_threshold(kept_row, "src/kept.rs"),
        )
        .expect("the fixture source is a declared magic class")
        .len(),
        COMMITTED_LITERALS.len(),
        "the unchanged file really does hold literals this scanner finds, so the \
         zero above is a delta against committed bytes rather than an empty scan"
    );
    assert_eq!(
        measured(magic_row(&report, "src/grown.rs"), "src/grown.rs"),
        1,
        "only the literal added since the commit is a regression"
    );
    // One regression is enough to fail the arm. The count is the finding here,
    // so there is no allowance for it to sit under: `magic.examine_threshold`
    // rides along on the measurement to record which literals were examined, and
    // reading it as a budget would admit as many new magic numbers as the
    // smallest one the policy bothers to look at.
    assert_eq!(
        arm_state(&report),
        BatteryState::Failed,
        "the arm fails on the one regression its own rows report"
    );
}

/// A file with no committed bytes is reported, not scanned against nothing.
///
/// The predecessor read a missing baseline as an empty one and counted every
/// literal in the file, which makes a new file's regression count a property of
/// how many literals it was born with. The row states the absence instead, and
/// names the commit it looked in, so the reason distinguishes a path `HEAD` does
/// not carry from a path nobody read.
///
/// The arm still reports for the tree: `src/kept.rs` measures beside this row,
/// because an inapplicable row may only speak for an arm when it speaks for
/// every row in it.
#[test]
fn a_path_absent_from_head_states_its_absence_instead_of_a_count() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(
        repository.join("src/kept.rs"),
        comparing_source(&COMMITTED_LITERALS),
    )
    .expect("fixture unchanged source");
    fixture_git(&repository, &["add", "--all"]);
    fixture_git(&repository, &["commit", "-q", "-m", "literals"]);

    fs::write(
        repository.join("src/born.rs"),
        comparing_source(&COMMITTED_LITERALS),
    )
    .expect("fixture uncommitted source");
    fixture_git(&repository, &["add", "src/born.rs"]);
    let head = fixture_git(&repository, &["rev-parse", "HEAD"]);

    let report = battery_report(&repository);

    let born = magic_row(&report, "src/born.rs");
    let MetricState::NotApplicable { reason } = born.state() else {
        panic!("a path with no committed bytes has no delta to take: {born:?}")
    };
    assert!(
        reason.contains("src/born.rs") && reason.contains(&head),
        "the row names the path and the commit it is absent from: {reason}"
    );
    assert_eq!(
        measured(magic_row(&report, "src/kept.rs"), "src/kept.rs"),
        0,
        "the paths that do have a baseline still measure beside it"
    );
}

/// A class this build cannot read is reported as that, not as a missing
/// baseline.
///
/// Both obstructions are true of `src/born.py` at once: it is in a class the
/// C-grammar scanner refuses by field, and `HEAD` does not carry it. `magic_state`
/// answers with the class, and this is the tree that separates the two orders
/// rather than merely exercising one. Every path in the unreadable class is
/// newly added, so under the opposite order no row would name the class at all,
/// and `src/kept.rs` measuring cleanly beside them would carry the whole arm to
/// `Passed` — a tree reporting its magic surface clean while none of it was read.
///
/// The Python is taken out of history rather than written fresh because
/// `production_repository` commits it. One left committed would keep naming the
/// class from its own row under either order, and both assertions below would
/// hold without the order ever being asked.
#[test]
fn an_unreadable_class_outranks_a_missing_baseline_on_the_same_row() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    for path in PREDECESSOR_CLASS_SOURCES {
        fixture_git(&repository, &["rm", "-q", "--cached", path]);
    }
    fs::write(
        repository.join("src/kept.rs"),
        comparing_source(&COMMITTED_LITERALS),
    )
    .expect("fixture readable neighbour");
    fixture_git(&repository, &["add", "src/kept.rs"]);
    fixture_git(&repository, &["commit", "-q", "-m", "literals, no python"]);

    // Born with a comparison-adjacent literal, so the row below is a refusal to
    // read the class rather than a scan that found nothing in it.
    fs::write(
        repository.join("src/born.py"),
        format!(
            "def born(value):\n    return value >= {}\n",
            INTRODUCED_LITERAL
        ),
    )
    .expect("fixture uncommitted source in the unreadable class");
    // Staged rather than merely written, because the selection is the index.
    for path in PREDECESSOR_CLASS_SOURCES {
        fixture_git(&repository, &["add", path]);
    }
    fixture_git(&repository, &["add", "src/born.py"]);

    let report = battery_report(&repository);

    let born = magic_row(&report, "src/born.py");
    let MetricState::Unsupported {
        source_field,
        reason,
        recovery,
        ..
    } = born.state()
    else {
        panic!("a class this build cannot read says so before anything about HEAD: {born:?}")
    };
    assert_eq!(
        source_field, "policy.languages.magic",
        "the refusal names the field that put this class in scope"
    );
    assert!(
        reason.contains(".py"),
        "the refusal names the class it cannot read: {reason}"
    );
    assert!(
        recovery.contains("compile an exact native parser"),
        "the refusal is actionable, which the absence it outranks is not: {recovery}"
    );
    assert!(
        !reason.contains("baseline"),
        "the durable obstruction is reported, not the one that expires at the \
         next commit: {reason}"
    );
    assert_eq!(
        measured(magic_row(&report, "src/kept.rs"), "src/kept.rs"),
        0,
        "the readable neighbour still measures, so the class refusal above is \
         this path's own state rather than the whole arm giving up"
    );
    assert_eq!(
        arm_state(&report),
        BatteryState::Unsupported,
        "one unreadable class in scope outranks its readable neighbour's clean \
         measurement, because a pass would claim the whole surface was read"
    );
}
