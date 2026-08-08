//! The coexistence record's citations resolve, or this test names the ones that
//! do not.
//!
//! `spice/COEXISTENCE.md` maps every integration point between the Python
//! predecessor and this port, and it is roughly sixty citations of other
//! people's files. Its three previous generations cited by line number and rotted
//! silently: by the time the record moved into the tree, one anchor had drifted
//! eleven lines and nothing had noticed. Citing by anchor only helps if something
//! resolves the anchors, which is this file.
//!
//! Predecessor citations are the interesting half. They name a tree that is not
//! this repository, so they cannot be resolved here and are asserted absent
//! instead -- a predecessor path that ever becomes a real file here would be a
//! reader resolving a citation against the wrong tree, which is worse than a
//! citation that plainly does not resolve.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// The record this test keeps honest.
const RECORD: &str = "spice/COEXISTENCE.md";

/// Path prefix naming the predecessor tree, which is not this repository.
const PREDECESSOR: &str = "spice-z/";

/// Extensions a citation's path may carry.
///
/// The scan is anchored on these rather than on "contains a slash". The record
/// is prose about two trees and writes plenty of code spans that name neither
/// file nor symbol, and a scanner that guessed would report those as rotted
/// citations and train a reader to ignore it.
const SOURCE_SUFFIXES: &[&str] = &[".rs", ".py", ".toml", ".md"];

/// The floor each half of the record must clear.
///
/// Well under what the record carries, because the number moves whenever a seam
/// is added or closed and a tight count would fail for the wrong reason. It is
/// here for the other failure: a scanner that stops matching still reports every
/// citation it found as resolved, and "no findings" over two citations reads
/// exactly like "no findings" over sixty.
const FLOOR: usize = 10;

/// Seam B's exact predecessor searches, as written in the record.
const STORE_SEARCHES: &[(&str, &str, Option<&str>)] = &[
    ("git grep -l spicemaxims", "spicemaxims", None),
    (
        "git grep -l spicemaxims -- tests/",
        "spicemaxims",
        Some("tests/"),
    ),
    ("git grep -l spicejournal", "spicejournal", None),
];

#[derive(Debug, Eq, PartialEq)]
enum StoreCountCheck {
    Checked,
    Skipped(String),
}

/// One `path#anchor` citation as the record writes it.
struct Citation {
    path: String,
    anchor: String,
}

#[test]
fn every_citation_in_the_coexistence_record_resolves() {
    let root = repository_root();
    let text = fs::read_to_string(root.join(RECORD))
        .unwrap_or_else(|error| panic!("{RECORD} is readable: {error}"));

    let cited = citations(&text);
    let predecessor = cited
        .iter()
        .filter(|citation| citation.path.starts_with(PREDECESSOR))
        .count();
    let port = cited.len() - predecessor;
    assert!(
        port >= FLOOR && predecessor >= FLOOR,
        "the scan found {port} port and {predecessor} predecessor citations, so \
         whatever it resolved below is not the record: both halves are the point"
    );

    let findings = unresolved(&root, &text);
    assert!(
        findings.is_empty(),
        "{} of {} citations in {RECORD} no longer resolve:\n{}",
        findings.len(),
        cited.len(),
        findings.join("\n")
    );
}

/// The resolver has to be able to fail, or the pass above says nothing.
///
/// Both directions, because the two halves fail for opposite reasons: a port
/// citation fails when the file or the anchor is gone, and a predecessor
/// citation fails when the path turns out to exist here after all. A resolver
/// that only ever checked the first would report the record clean while a
/// reader was being pointed at the wrong tree.
#[test]
fn the_resolver_reports_a_rotted_citation_in_either_half() {
    let root = repository_root();

    let missing_file = unresolved(&root, "`spice/src/no_such_module.rs#anything`");
    assert_eq!(
        missing_file.len(),
        1,
        "a citation naming no file must be reported: {missing_file:?}"
    );

    let missing_anchor = unresolved(
        &root,
        "`spice/src/repository.rs#a_symbol_this_file_does_not_declare`",
    );
    assert_eq!(
        missing_anchor.len(),
        1,
        "a citation whose anchor left the file must be reported: {missing_anchor:?}"
    );

    // The predecessor half only fires in a tree that has the cited path, which
    // this repository must not have, so it is driven under a scratch root that
    // does. Both spellings run against that same root: the only difference
    // between them is whether the cited file is there.
    let scratch = scratch_root();
    fs::create_dir(scratch.join(PREDECESSOR.trim_end_matches('/')))
        .expect("the scratch predecessor directory is created");
    fs::write(scratch.join("spice-z/present.rs"), "fn cited() {}\n")
        .expect("the scratch predecessor file is written");

    let absent = unresolved(&scratch, "`spice-z/absent.rs#cited`");
    assert!(
        absent.is_empty(),
        "a predecessor path with nothing at it is the ordinary case and must \
         pass, or the report below is unconditional: {absent:?}"
    );
    let present = unresolved(&scratch, "`spice-z/present.rs#cited`");
    assert_eq!(
        present.len(),
        1,
        "a predecessor path that resolves here must be reported: {present:?}"
    );

    fs::remove_dir_all(&scratch).expect("the scratch root is removed");
}

/// The record must not slide back into citing by line.
///
/// Every anchor resolving today is a fact about this revision, not a property of
/// the record: nothing above stops the next correction from writing
/// `repository.rs:468` beside the anchors, and a line number is precisely what
/// the resolver cannot check. It is prose to every scanner and wrong to every
/// reader the moment the file above it grows a line, which is how three
/// generations of this record rotted without a single failing test.
#[test]
fn the_record_cites_no_line_numbers() {
    let root = repository_root();
    let text = fs::read_to_string(root.join(RECORD))
        .unwrap_or_else(|error| panic!("{RECORD} is readable: {error}"));

    let cited = line_citations(&text);
    assert!(
        cited.is_empty(),
        "{RECORD} cites {} line number(s); cite the symbol or literal at that \
         line instead, so the resolver can check it: {}",
        cited.len(),
        cited.join(", ")
    );

    // The scan has to be able to see one, or the emptiness above is the scan's
    // rather than the record's. Every extension the record may cite is driven,
    // because a scan that lost one of them would still answer empty here.
    for suffix in SOURCE_SUFFIXES {
        let planted = format!("`spice/src/repository{suffix}:468` and prose after it");
        assert_eq!(
            line_citations(&planted),
            vec![format!("spice/src/repository{suffix}:468")],
            "a {suffix} line citation must be seen for the record's silence to mean anything"
        );
    }
}

#[test]
fn seam_b_store_counts_match_the_predecessor_tree() {
    let root = repository_root();
    let text = fs::read_to_string(root.join(RECORD))
        .unwrap_or_else(|error| panic!("{RECORD} is readable: {error}"));
    let predecessor = root
        .parent()
        .expect("the repository has a parent holding sibling worktrees")
        .join(PREDECESSOR.trim_end_matches('/'));

    match check_store_counts(&text, &predecessor) {
        StoreCountCheck::Checked => {}
        StoreCountCheck::Skipped(reason) => eprintln!("SKIPPED: {reason}"),
    }
}

#[test]
fn an_absent_predecessor_tree_is_an_explicit_store_count_skip() {
    let root = scratch_root();
    let absent = root.join("absent-spice-z");
    let text = STORE_SEARCHES
        .iter()
        .map(|(search, _, _)| format!("| `{search}` | 0 |"))
        .collect::<Vec<_>>()
        .join("\n");

    let StoreCountCheck::Skipped(reason) = check_store_counts(&text, &absent) else {
        panic!("an absent predecessor tree must be an explicit skip")
    };
    assert!(
        reason.contains(&absent.display().to_string()),
        "the skip must name the absent predecessor tree: {reason}"
    );

    fs::remove_dir_all(&root).expect("the scratch root is removed");
}

fn check_store_counts(text: &str, predecessor: &Path) -> StoreCountCheck {
    let recorded = STORE_SEARCHES
        .iter()
        .map(|(search, _, _)| (*search, recorded_store_count(text, search)))
        .collect::<Vec<_>>();

    if !predecessor.exists() {
        return StoreCountCheck::Skipped(format!(
            "Seam B store counts were not checked because the predecessor tree is absent at {}",
            predecessor.display()
        ));
    }
    assert!(
        predecessor.is_dir(),
        "the predecessor tree path is not a directory: {}",
        predecessor.display()
    );

    for ((search, needle, scope), (_, expected)) in STORE_SEARCHES.iter().zip(recorded) {
        let actual = git_grep_file_count(predecessor, needle, *scope);
        assert_eq!(
            actual,
            expected,
            "Seam B records {expected} files for `{search}` but the predecessor tree at {} has {actual}",
            predecessor.display()
        );
    }
    StoreCountCheck::Checked
}

fn recorded_store_count(text: &str, search: &str) -> usize {
    let prefix = format!("| `{search}` | ");
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "{RECORD} must contain exactly one count row for `{search}`"
    );
    rows[0]
        .strip_suffix(" |")
        .unwrap_or_else(|| panic!("{RECORD}'s `{search}` row must end with a table cell"))
        .parse()
        .unwrap_or_else(|error| panic!("{RECORD}'s `{search}` count is an integer: {error}"))
}

fn git_grep_file_count(root: &Path, needle: &str, scope: Option<&str>) -> usize {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(["grep", "-l", needle]);
    if let Some(scope) = scope {
        command.args(["--", scope]);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("`git grep` launches for {}: {error}", root.display()));
    match output.status.code() {
        Some(0) => {}
        Some(1) if output.stdout.is_empty() && output.stderr.is_empty() => return 0,
        status => panic!(
            "`git grep -l {needle}` refused for {} with {status:?}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("`git grep` paths are UTF-8: {error}"))
        .lines()
        .count()
}

/// Every `path:line` citation the record writes, which must be none.
fn line_citations(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for suffix in SOURCE_SUFFIXES {
        let needle = format!("{suffix}:");
        for (at, _) in text.match_indices(&needle) {
            let after = &text[at + needle.len()..];
            if !after.starts_with(|character: char| character.is_ascii_digit()) {
                continue;
            }
            let start = text[..at]
                .rfind(['`', ' ', '\n'])
                .map_or(0, |boundary| boundary + 1);
            let digits = after
                .find(|character: char| !character.is_ascii_digit())
                .unwrap_or(after.len());
            found.push(text[start..at + needle.len() + digits].to_owned());
        }
    }
    found
}

/// An exclusive scratch root.
///
/// The predecessor half of the resolver can only be exercised in a tree that
/// carries a `spice-z/` path, and this repository carrying one is the very
/// thing being guarded against.
fn scratch_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-coexistence-record-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).expect("exclusive scratch root is created");
    root
}

/// Every citation the record writes, in the order it writes them.
fn citations(text: &str) -> Vec<Citation> {
    text.split('`')
        .filter_map(|span| {
            let (path, anchor) = span.split_once('#')?;
            if anchor.is_empty() || !SOURCE_SUFFIXES.iter().any(|suffix| path.ends_with(suffix)) {
                return None;
            }
            Some(Citation {
                path: path.to_owned(),
                anchor: anchor.to_owned(),
            })
        })
        .collect()
}

/// Each citation that does not resolve, said the way a reader can act on.
fn unresolved(root: &Path, text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    for citation in citations(text) {
        let full = root.join(&citation.path);
        if let Some(relative) = citation.path.strip_prefix(PREDECESSOR) {
            if full.exists() {
                findings.push(format!(
                    "{}#{}: names the predecessor tree but exists in this one; \
                     drop the {PREDECESSOR} prefix if the citation meant {relative}",
                    citation.path, citation.anchor
                ));
            }
            continue;
        }
        let Ok(source) = fs::read_to_string(&full) else {
            findings.push(format!(
                "{}#{}: no readable file at that path",
                citation.path, citation.anchor
            ));
            continue;
        };
        if !source.contains(&citation.anchor) {
            findings.push(format!(
                "{}#{}: the file no longer contains that anchor",
                citation.path, citation.anchor
            ));
        }
    }
    findings
}

/// The repository root, from this crate's manifest rather than the process
/// directory, which the harness does not promise.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the spice crate sits one level under the repository root")
        .to_owned()
}
