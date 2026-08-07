//! What a Markdown-family refusal tells the reader it refused.
//!
//! Every assertion here reads the encoded evidence rather than the scanner that
//! produced it. The scanners have always known the citing line and the target;
//! the defect was that the acquisition threw both away one call from the report,
//! so a test that asked the scanner would have passed before this arm carried a
//! finding at all.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{markdown_state, rust_doc_markdown_state};
use crate::metrics::{
    EvidenceState, MEASUREMENT_FINDING_LIMIT, MetricEvidence, MetricId, MetricKey,
    MetricMeasurement, ParserEvidence, ParserId,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// A scratch tree that removes itself, so one arm's fixture cannot reach another.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(label: &str) -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gate-markdown-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create markdown fixture root");
        Self { root }
    }

    fn write(&self, path: &str, text: &str) {
        let target = self.root.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(target, text).expect("write fixture source");
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The encoded evidence for one state, which is the only thing a reader gets.
fn encoded(metric: MetricId, path: &str, state: EvidenceState) -> String {
    let evidence = MetricEvidence::new(metric, MetricKey::path(path).expect("metric key"), state)
        .expect("markdown metric evidence");
    serde_json::to_string(&evidence).expect("evidence encodes")
}

fn measured(state: &EvidenceState) -> &MetricMeasurement {
    match state {
        EvidenceState::Measured { measurement } => measurement,
        other => panic!("a planted breach must measure: {other:?}"),
    }
}

/// A planted citation names its line and its unresolved target in the report.
///
/// The plant reproduces the shape that exposed the defect: a doc comment whose
/// third line cites a Markdown document the inventory does not carry. That name
/// is spelled in the fixture below and never in this prose, because this arm
/// measures doc comments: naming a phantom document here would refuse the very
/// file that proves the naming works. Before this arm carried findings the same
/// plant encoded the value `1` and nothing else, so the reader was told a file
/// had one absence and had to search the file to learn which citation was
/// phantom.
#[test]
fn a_planted_citation_names_its_line_and_target_in_the_encoded_report() {
    let tree = Tree::new("absence");
    let source = "gate/tests/retired_names.rs";
    tree.write(
        source,
        "//! The retirement ledger's jurisdiction.\n\
         //!\n\
         //! The deny-list is scoped by NAMES.md and nothing else.\n\
         pub const OWNED: u8 = 0;\n",
    );
    let tracked = vec![source.to_owned()];

    let state = rust_doc_markdown_state(&tree.root, source, &tracked)
        .expect("a source carrying a citation contributes a row");
    assert_eq!(
        measured(&state).value(),
        1,
        "one phantom citation is one absence"
    );

    let report = encoded(MetricId::RustDocMarkdownTargetAbsences, source, state);
    assert!(
        report.contains("NAMES.md"),
        "the unresolved target is named in the report: {report}"
    );
    assert!(
        report.contains(source),
        "the citing source is named in the report: {report}"
    );
    assert!(
        report.contains("\"line\":3"),
        "the citing line is named in the report: {report}"
    );
    // An absence has no correct spelling to offer, and saying so is not the same
    // as saying nothing: the target is untracked, not merely miscased.
    assert!(
        !report.contains("\"expected\""),
        "an absence offers no expected spelling: {report}"
    );
}

/// A case mismatch names both the spelling written and the spelling tracked.
///
/// This is the half the parent defect had held longer: the case arm never named
/// the mislinked target either. Naming only the wrong spelling would leave the
/// reader to guess the right one, so the report carries the pair.
#[test]
fn a_case_mismatch_names_the_written_target_and_the_tracked_spelling() {
    let tree = Tree::new("case");
    let source = "docs/guide.md";
    tree.write(source, "See [the note](Target.md) for the rule.\n");
    tree.write("docs/target.md", "the tracked spelling\n");
    let tracked = vec![source.to_owned(), "docs/target.md".to_owned()];

    let state = markdown_state(&tree.root, source, &tracked);
    assert_eq!(
        measured(&state).value(),
        1,
        "one miscased link is one mismatch"
    );

    let report = encoded(MetricId::MarkdownLinkCaseMismatches, source, state);
    assert!(
        report.contains("Target.md"),
        "the target as written is named, because that is what a reader searches for: {report}"
    );
    assert!(
        report.contains("docs/target.md"),
        "the tracked spelling the link should have used is named: {report}"
    );
    assert!(
        report.contains("\"line\":1"),
        "the mislinking line is named in the report: {report}"
    );
}

/// One pathological file cannot set the size of the whole report.
///
/// The value keeps the true total while the listing stops at the bound, so the
/// two disagreeing is what tells a reader the listing was shortened. A bound
/// that also lowered the value would hide the breach instead of shortening it.
#[test]
fn a_pathological_file_carries_a_bounded_listing_beside_an_unbounded_count() {
    let tree = Tree::new("bound");
    let source = "docs/many.md";
    let planted = MEASUREMENT_FINDING_LIMIT + 9;
    let mut text = String::new();
    let mut tracked = vec![source.to_owned()];
    for index in 0..planted {
        text.push_str(&format!("[link {index}](Note{index}.md)\n"));
        tracked.push(format!("docs/note{index}.md"));
        tree.write(&format!("docs/note{index}.md"), "tracked\n");
    }
    tree.write(source, &text);

    let state = markdown_state(&tree.root, source, &tracked);
    let measurement = measured(&state);
    assert_eq!(
        measurement.value(),
        planted as u64,
        "the count stays whole: a bound on the listing is not a bound on the breach"
    );
    assert_eq!(
        measurement.findings().len(),
        MEASUREMENT_FINDING_LIMIT,
        "the listing stops at the bound stated where it is enforced"
    );

    let report = encoded(MetricId::MarkdownLinkCaseMismatches, source, state);
    assert_eq!(
        report.matches("\"target\"").count(),
        MEASUREMENT_FINDING_LIMIT,
        "the encoded report carries exactly the bounded listing: {report}"
    );
}

/// A zero-tolerance breach that names nothing is not an encodable report.
///
/// This is what makes the naming structural rather than merely present. Without
/// it the acquisition could quietly stop attaching findings and every assertion
/// above would still hold for whatever code still did attach them.
#[test]
fn a_breach_naming_no_finding_is_refused_rather_than_encoded() {
    for metric in [
        MetricId::MarkdownLinkCaseMismatches,
        MetricId::RustDocMarkdownTargetAbsences,
    ] {
        let anonymous = MetricMeasurement::new(
            1,
            ParserEvidence::Native {
                parser: ParserId::MarkdownTrackedTargetsV2,
            },
        );
        let error = MetricEvidence::new(
            metric,
            MetricKey::path("docs/anonymous.md").expect("metric key"),
            EvidenceState::measured(anonymous),
        )
        .expect_err("a breach that names nothing cannot be encoded");
        assert!(
            error.to_string().contains("must name at least one finding"),
            "the refusal says what is missing: {error}"
        );
    }
}
