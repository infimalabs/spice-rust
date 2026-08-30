//! The claim-warning provenance record keeps its current-tree citations live
//! without making a sibling predecessor checkout part of the test fixture.

use std::fs;
use std::path::{Path, PathBuf};

const RECORD: &str = "spice/CLAIM_WARNING_PROVENANCE.md";
const PREDECESSOR_PREFIX: &str = "spice-z/";
const PREDECESSOR_REVISION: &str = "8119859e21eaff5058274473eaa8b56a144eea3a";
const RUST_REVISION: &str = "8037bd652e2cb40f2323f51332a8ccb5cef8d404";
const SOURCE_SUFFIXES: &[&str] = &[".rs", ".py"];
const CITATION_FLOOR: usize = 8;

#[derive(Debug)]
struct Citation {
    path: String,
    anchor: String,
}

#[test]
fn every_current_citation_resolves() {
    let root = repository_root();
    let text = record(&root);
    let citations = citations(&text);
    let predecessor = citations
        .iter()
        .filter(|citation| citation.path.starts_with(PREDECESSOR_PREFIX))
        .count();
    let current = citations.len() - predecessor;
    assert!(
        predecessor >= CITATION_FLOOR && current >= CITATION_FLOOR,
        "citation scan found only {predecessor} predecessor and {current} current sources"
    );

    let unresolved = citations
        .iter()
        .filter_map(|citation| unresolved(&root, citation))
        .collect::<Vec<_>>();
    assert!(
        unresolved.is_empty(),
        "{} citation(s) in {RECORD} no longer resolve:\n{}",
        unresolved.len(),
        unresolved.join("\n")
    );
}

#[test]
fn predecessor_evidence_is_pinned_without_becoming_a_runtime_input() {
    let root = repository_root();
    let text = record(&root);
    assert!(
        text.contains(PREDECESSOR_REVISION),
        "record pins the inspected predecessor"
    );
    assert!(
        text.contains(RUST_REVISION),
        "record pins the inspected Rust tree"
    );

    for required in [
        "spice-z/spice/agent/wrap.py#class AgentWorkingStateInjector",
        "spice-z/spice/agent/wrap.py#def _working_state_claim(",
        "spice-z/spice/agent/sidechannel.py#class AgentSideChannelServer",
        "spice-z/spice/agent/supervisorwatch.py#def renew_supervised_claim(",
        "spice-z/tests/test_agentcli.py#def test_working_state_claim_lease_warnings",
        "spice-z/tests/test_sidechannel.py#def test_side_channel_repeats_critical_claim_warning",
    ] {
        assert!(
            text.contains(required),
            "record retains predecessor citation {required}"
        );
    }

    assert!(
        !root.join(PREDECESSOR_PREFIX.trim_end_matches('/')).exists(),
        "the current repository must not carry predecessor source as a test or runtime fallback"
    );
}

#[test]
fn the_record_uses_symbol_anchors_instead_of_line_numbers() {
    let text = record(&repository_root());
    for suffix in SOURCE_SUFFIXES {
        assert!(
            !text.contains(&format!("{suffix}:")),
            "{RECORD} contains a {suffix}:line spelling; cite a stable source anchor"
        );
    }
}

fn record(root: &Path) -> String {
    fs::read_to_string(root.join(RECORD))
        .unwrap_or_else(|error| panic!("{RECORD} is readable: {error}"))
}

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

fn unresolved(root: &Path, citation: &Citation) -> Option<String> {
    if citation.path.starts_with(PREDECESSOR_PREFIX) {
        return root.join(&citation.path).exists().then(|| {
            format!(
                "{}#{} exists in the current tree; predecessor evidence became a local input",
                citation.path, citation.anchor
            )
        });
    }
    let path = root.join(&citation.path);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            return Some(format!(
                "{}#{} has no readable current source: {error}",
                citation.path, citation.anchor
            ));
        }
    };
    (!source.contains(&citation.anchor)).then(|| {
        format!(
            "{}#{} no longer names source text",
            citation.path, citation.anchor
        )
    })
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the spice crate is one level below the repository root")
        .to_owned()
}
