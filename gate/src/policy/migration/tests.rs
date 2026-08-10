//! Who a repository-document inventory's refusal belongs to.
//!
//! The inventory has two authors: a repository that declares one, and this
//! release that packages a default for the repositories that declare none. Both
//! reach one normalization, so the thing worth pinning is not that a bad path is
//! refused — it is which of the two the refusal sends an operator to.
//!
//! Every case below enters through `repo_truth_inventory` rather than through
//! the normalization underneath it. Reaching the packaged spelling by any other
//! door proves the spelling and not the branch, and the branch is the defect:
//! routing the packaged inventory back through the tracked source produces
//! exactly the refusal this module exists to prevent, while a normalization
//! tested on its own stays green throughout.

use super::*;

/// A path no repository could have written and none can normalize.
///
/// It escapes the repository root, which is the spelling `normalize_path`
/// refuses, so every refusal below is produced by the real normalization rather
/// than by a check written for this test.
const ESCAPING_DOC: &str = "../outside.md";

/// The tracked document a declaring repository is refused at.
const TRACKED_SOURCE: &str = "spice-rust.toml";

/// A packaged default that fails normalization is refused as packaged.
///
/// The bad inventory is handed to the same resolution a repository's own goes
/// through, with nothing declared, so what is measured is the branch a
/// repository actually reaches when it declares no documents. A packaged path
/// is unreachable from every tracked byte, so a repository handed this refusal
/// has nothing to edit and the recovery is a release fix.
///
/// The tracked source is asserted absent from both halves of the error rather
/// than from the field alone, because naming `spice-rust.toml` as the document while
/// the field says otherwise sends the same operator to the same wrong file.
#[test]
fn a_packaged_document_that_fails_normalization_is_refused_as_packaged() {
    let error = repo_truth_inventory(None, &[ESCAPING_DOC], TRACKED_SOURCE)
        .expect_err("a path escaping the repository is not a repository-relative spelling");

    assert_eq!(
        error.source(),
        PACKAGED_POLICY_SOURCE,
        "a packaged default is attributed to the release that packages it"
    );
    assert_eq!(
        error.field(),
        PACKAGED_REPO_TRUTH_FIELD,
        "and to the packaged inventory itself, which is the only thing a fix can touch"
    );
    assert!(
        !error.source().contains(TRACKED_SOURCE)
            && !error.field().contains(DECLARED_REPO_TRUTH_FIELD),
        "no tracked field can be named for bytes no repository wrote: {} at {}",
        error.field(),
        error.source()
    );
    assert!(
        error.reason().contains(ESCAPING_DOC),
        "the refusal still names the path that failed: {}",
        error.reason()
    );
}

/// A declared inventory keeps naming the field the repository spelled it in.
///
/// Asserted beside the packaged case because separating the two attributions is
/// the whole change: a fix that moved both halves onto the packaged source would
/// satisfy the test above on its own, and would then send every repository with
/// a genuine typo to this release's source instead of to its own file.
#[test]
fn a_declared_document_that_fails_normalization_is_refused_at_its_field() {
    let declared = vec![ESCAPING_DOC.to_owned()];

    let error = repo_truth_inventory(Some(&declared), DEFAULT_REPO_TRUTH_DOCS, TRACKED_SOURCE)
        .expect_err("a path escaping the repository is not a repository-relative spelling");

    assert_eq!(
        error.source(),
        TRACKED_SOURCE,
        "a repository's own spelling is refused at the repository's own document"
    );
    assert_eq!(
        error.field(),
        DECLARED_REPO_TRUTH_FIELD,
        "and at the field that spelled it"
    );
}

/// Every packaged default normalizes, and it is the inventory named here.
///
/// The inventory is written out rather than read off the constant, because a
/// test that loops over `DEFAULT_REPO_TRUTH_DOCS` alone agrees with any content
/// it happens to hold: emptying it would leave nothing to iterate and pass, and
/// an empty fallback leaves a repository measuring no documents while reading
/// exactly like one that had none to measure. The literal fails when a default
/// is dropped; the resolution fails when one that cannot normalize is added.
#[test]
fn every_packaged_default_document_is_a_repository_relative_spelling() {
    assert_eq!(
        DEFAULT_REPO_TRUTH_DOCS,
        &["AGENTS.md"],
        "this release packages exactly one repository-truth document"
    );

    let resolved = repo_truth_inventory(None, DEFAULT_REPO_TRUTH_DOCS, TRACKED_SOURCE)
        .expect("every packaged default is a repository-relative spelling");

    assert_eq!(
        resolved,
        vec!["AGENTS.md".to_owned()],
        "a layer declaring nothing resolves the packaged inventory, spelled as packaged"
    );
}
