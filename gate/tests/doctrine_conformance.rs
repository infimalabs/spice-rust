//! Corpus conformance for the doctrine of record.
//!
//! The root `DOCTRINE.md` is the durable doctrine of record. These checks hold
//! its kernel and decision records to settled requirements.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The doctrine of record.
const DOCTRINE: &str = "DOCTRINE.md";

/// Every settled kernel invariant, one paragraph each.
const KERNEL_INVARIANTS: &[&str] = &[
    "Transcript",
    "Board",
    "Stigmergy",
    "Authority",
    "Break",
    "Refusal",
    "Wake",
    "Channels",
    "Outcomes",
    "Evidence",
];

/// The five settled decision records.
const DECISION_RECORDS: &[&str] = &[
    "One authority",
    "Principals & cooperative threat model",
    "Three clocks",
    "The target manifest",
    "Resident life",
];

/// Language that marks a question rather than a settled decision.
///
/// Decision records contain settled law rather than open questions.
const UNSETTLED: &[&str] = &[
    "TBD",
    "TODO",
    "open question",
    "to be decided",
    "to be determined",
    "we will decide",
    "still undecided",
];

#[test]
fn the_kernel_states_every_invariant_as_law() {
    let doctrine = read_doctrine();
    let kernel = section(&doctrine, "## The kernel");

    for invariant in KERNEL_INVARIANTS {
        let lead = format!("**{invariant}.**");
        let paragraph = kernel
            .split("\n\n")
            .find(|paragraph| paragraph.trim_start().starts_with(&lead))
            .unwrap_or_else(|| panic!("the kernel must state the {invariant} invariant"));

        assert!(
            paragraph.trim().len() > lead.len() + 80,
            "the {invariant} invariant must be stated, not merely named"
        );
    }

    let stated = kernel
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("**"))
        .filter_map(|line| line.split_once(".**"))
        .map(|(name, _)| name.to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        stated,
        KERNEL_INVARIANTS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>(),
        "the kernel must state exactly the invariants SCAFFOLD names"
    );
}

#[test]
fn every_decision_record_carries_rationale_and_explicit_deletions() {
    let doctrine = read_doctrine();
    let records = section(&doctrine, "## The decision records");

    assert_eq!(
        headings(records, "### "),
        DECISION_RECORDS
            .iter()
            .map(|record| (*record).to_owned())
            .collect::<Vec<_>>(),
        "the five settled decision records must each be present"
    );

    for record in DECISION_RECORDS {
        let body = section(records, &format!("### {record}"));
        let paragraphs = body
            .split("\n\n")
            .map(str::trim)
            .filter(|paragraph| !paragraph.is_empty() && !paragraph.starts_with("###"))
            .collect::<Vec<_>>();

        assert!(
            paragraphs.len() >= 3,
            "the {record} record needs its resolution, its rationale, and its \
             deletions; found {} paragraph(s)",
            paragraphs.len()
        );
        assert!(
            paragraphs
                .iter()
                .any(|paragraph| paragraph.contains("delete")),
            "the {record} record must name what it deleted; a decision that \
             removes nothing settled nothing"
        );
    }
}

#[test]
fn the_doctrine_reads_as_settled_law() {
    let doctrine = read_doctrine();
    let lowered = doctrine.to_lowercase();

    for marker in UNSETTLED {
        assert!(
            !lowered.contains(&marker.to_lowercase()),
            "DOCTRINE.md must record decisions, never questions, but it \
             contains {marker:?}"
        );
    }
}

#[test]
fn doctrine_links_resolve() {
    let root = workspace_root();
    let doctrine = read_doctrine();

    for target in relative_links(&doctrine) {
        assert!(
            root.join(&target).exists(),
            "DOCTRINE.md links to {target}, which does not exist"
        );
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate must be a direct workspace member")
        .to_path_buf()
}

fn read_doctrine() -> String {
    fs::read_to_string(workspace_root().join(DOCTRINE)).expect("DOCTRINE.md must be readable")
}

/// Every heading at `prefix`, in document order, with the prefix stripped.
fn headings(text: &str, prefix: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .map(|heading| heading.trim().to_owned())
        .collect()
}

/// The body under `heading`, ending at the next heading of the same depth.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let depth = heading
        .split_once(' ')
        .map(|(hashes, _)| hashes.len())
        .expect("a heading names its depth");
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("DOCTRINE.md must contain {heading:?}"));
    let body = &text[start + heading.len()..];
    let boundary = format!("\n{} ", "#".repeat(depth));

    body.find(&boundary).map_or(body, |end| &body[..end])
}

/// Relative markdown link targets, ignoring anchors and absolute schemes.
fn relative_links(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;

    while let Some(open) = rest.find("](") {
        let after = &rest[open + 2..];
        let Some(close) = after.find(')') else {
            break;
        };
        let target = &after[..close];
        rest = &after[close..];

        if target.starts_with('#') || target.contains("://") || target.starts_with("mailto:") {
            continue;
        }
        let target = target.split('#').next().unwrap_or_default();
        if !target.is_empty() {
            targets.push(target.to_owned());
        }
    }

    targets
}
