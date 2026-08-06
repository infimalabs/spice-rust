//! Repository guard for hand-written wire-response fixtures.
//!
//! App tests exercise socket admission with encoded bytes, but valid replies
//! must be assembled from typed wire DTOs first. The wire crate keeps one raw
//! response document because that test owns the external JSON spelling.

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn app_response_fixtures_have_no_hand_written_outcome_field() {
    let marker = format!("\"{}\"", "outcome");
    let root = workspace_root();
    let mut hits = quoted_outcome_hits(&root.join("app/tests"), &marker);
    hits.retain(|(path, _)| {
        path.file_name()
            .is_none_or(|name| name != "response_fixture_inventory.rs")
    });

    assert!(
        hits.is_empty(),
        "valid app response fixtures must use support::response: {hits:?}"
    );
}

#[test]
fn wire_keeps_only_its_justified_external_response_document_raw() {
    let marker = format!("\"{}\"", "outcome");
    let root = workspace_root();
    let hits = quoted_outcome_hits(&root.join("wire/tests"), &marker);
    assert_eq!(
        hits,
        vec![(root.join("wire/tests/roundtrip.rs"), 1)],
        "raw wire response documents need an explicit format-test justification"
    );

    let roundtrip = fs::read_to_string(root.join("wire/tests/roundtrip.rs"))
        .expect("read wire round-trip fixtures");
    for guard in [
        "proves the exact external JSON spelling",
        "caught at compile time by the typed `Response<DoneResult>` literals",
    ] {
        assert!(
            roundtrip.contains(guard),
            "missing raw-fixture guard: {guard}"
        );
    }
}

fn quoted_outcome_hits(directory: &Path, marker: &str) -> Vec<(PathBuf, usize)> {
    let mut paths = rust_sources(directory);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read Rust test source");
            let count = source.matches(marker).count();
            (count > 0).then_some((path, count))
        })
        .collect()
}

fn rust_sources(directory: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    for entry in fs::read_dir(directory).expect("read test directory") {
        let path = entry.expect("read test entry").path();
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push(path);
        }
    }
    sources
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("app has a workspace parent")
        .to_path_buf()
}
