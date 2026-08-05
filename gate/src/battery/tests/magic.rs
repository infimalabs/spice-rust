//! The magic-number arm measured against real committed bytes.
//!
//! Every assertion here rests on the same repository, because the three cases
//! only mean anything beside each other: one file that was committed with
//! literals and left alone, one that was committed with literals and given one
//! more, and one that has no committed bytes at all. A scan handed no baseline
//! reports the same nonzero count for all three, so a test that showed only the
//! third — or only the first — could not tell a delta from a plain scan.

use super::*;
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricKey};

/// The literals the fixture's committed sources already hold.
///
/// Each clears the packaged `magic.examine_threshold` of 10 on its own, so the
/// scanner has something to find in every one of these files before any change
/// is made to them. A file whose literals were all below that bar would report
/// zero regressions whether or not its baseline was ever read.
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

/// The value one row measured, or a panic naming the state it carried instead.
fn measured(evidence: &MetricEvidence, path: &str) -> u64 {
    match evidence.state() {
        MetricState::Measured { measurement } => measurement.value(),
        state => panic!("{path:?} was expected to measure, but carried {state:?}"),
    }
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

    assert_eq!(
        measured(magic_row(&report, "src/kept.rs"), "src/kept.rs"),
        0,
        "an unchanged file's committed literals are not regressions"
    );
    assert_eq!(
        crate::metrics::scan_magic_regressions("src/kept.rs", &kept, None, 10)
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

/// Run one complete battery over a repository and return its report.
fn battery_report(repository: &Path) -> BatteryReport {
    let mut capture = crate::BatteryCapture::capture(
        repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        crate::ActorCapture::Acting {
            actor: "magic-actor".to_owned(),
            bound_claims: Vec::new(),
        },
        crate::RimCapture::new(repository, None, Vec::new())
            .expect("the fixture repository root is absolute"),
        crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("hygiene roots"),
    )
    .expect("capture the fixture battery source");
    capture.execute().expect("complete fixture battery")
}
