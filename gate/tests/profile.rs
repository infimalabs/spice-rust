//! Current compiled execution profiles exercised through their production API.

use std::fs;

use gate::{CampStanding, Contract, LONG_LINE_LIMIT, Standing};

mod support;

use support::{Tree, git_commit, git_repository};

const BASELINE: &str = "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n";
const LOCI: [&str; 4] = [
    "gate/src/camp.rs",
    "gate/src/input.rs",
    "gate/src/kill.rs",
    "gate/src/verdict.rs",
];

#[test]
fn production_contract_distinguishes_clean_and_camped_complete_populations() {
    let clean = profile_tree("profile-clean", [10, 20, 30, 40]);
    let clean_before = tracked_bytes(&clean);
    let contract = Contract::load(&clean.root).expect("clean current contract loads");
    let report = contract
        .audit_camp(&clean.root)
        .expect("clean Camp profile completes");
    assert_eq!(report.standing, CampStanding::Clean);
    assert_eq!(report.distributions[0].population, 4);
    assert_eq!(tracked_bytes(&clean), clean_before);

    let camped = profile_tree("profile-camped", [95, 100, 105, 106]);
    let camped_before = tracked_bytes(&camped);
    let contract = Contract::load(&camped.root).expect("camped current contract loads");
    let ordinary = contract
        .adjudicate(&camped.root)
        .expect("ordinary profile completes on the same caller tree");
    let report = contract
        .audit_camp(&camped.root)
        .expect("camped Camp profile completes");

    assert_eq!(ordinary.receipt().standing(), Standing::Clean);
    assert_eq!(ordinary.receipt().gate_set(), &contract.gate_set().digest());
    assert_eq!(report.standing, CampStanding::Camped);
    assert_eq!(report.distributions[0].population, 4);
    assert_eq!(report.distributions[0].near, 3);
    assert_eq!(tracked_bytes(&camped), camped_before);
    assert!(
        !camped.path(".spice-rs").exists(),
        "the profiles wrote a journal"
    );
}

fn profile_tree(label: &str, lengths: [usize; 4]) -> Tree {
    let tree = git_repository(label);
    tree.write("spice.toml", gate::NARROWEST_FILE_CLASSES);
    tree.write("baseline.toml", BASELINE);
    for (path, length) in LOCI.into_iter().zip(lengths) {
        assert!(length <= LONG_LINE_LIMIT + 6);
        tree.write(path, &("x".repeat(length) + "\n"));
    }
    git_commit(&tree, "profile fixture");
    tree
}

fn tracked_bytes(tree: &Tree) -> Vec<(String, Vec<u8>)> {
    ["spice.toml", "baseline.toml"]
        .into_iter()
        .chain(LOCI)
        .map(|path| {
            (
                path.to_owned(),
                fs::read(tree.path(path)).expect("fixture path remains readable"),
            )
        })
        .collect()
}
