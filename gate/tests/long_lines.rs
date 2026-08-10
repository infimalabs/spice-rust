//! Current-contract coverage for per-file long-line attribution against `HEAD`.

use gate::{Contract, GateReport, LONG_LINE_LIMIT, Standing};

mod support;

use support::{Tree, git, git_commit, git_repository};

const BASELINE: &str = "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n";

#[test]
fn equal_tree_totals_cannot_cancel_across_files() {
    let repository = fixture("long-lines-cancellation");
    repository.write("src/remove.rs", &format!("{}\n", long("removed")));
    repository.write("src/add.rs", "short\n");
    git_commit(&repository, "one inherited long line");

    repository.write("src/remove.rs", "short\n");
    repository.write("src/add.rs", &format!("short\n{}\n", long("introduced")));

    let run = adjudicate(&repository);
    assert_eq!(run.receipt().standing(), Standing::Refused);
    assert_eq!(
        run.receipt().head(),
        git(&repository, &["rev-parse", "HEAD"])
    );
    assert_eq!(run.receipt().long_lines().len(), 1);
    let finding = &run.receipt().long_lines()[0];
    assert_eq!((finding.path(), finding.line()), ("src/add.rs", 2));
    assert_eq!((finding.baseline_debt(), finding.current_debt()), (0, 1));
    let report = run.report();
    assert!(
        report
            .render()
            .contains("finding rust.long-lines \"src/add.rs\":2")
    );
    let encoded = report.encode().expect("encode attributed report");
    assert_eq!(
        GateReport::decode(&encoded).expect("decode attributed report"),
        report
    );
}

#[test]
fn changed_files_keep_their_unchanged_inherited_debt() {
    let repository = fixture("long-lines-inherited");
    let inherited = long("inherited");
    repository.write("src/lib.rs", &format!("{inherited}\nshort\n"));
    git_commit(&repository, "inherited long line");

    repository.write("src/lib.rs", &format!("{inherited}\nchanged short\n"));

    let run = adjudicate(&repository);
    assert_eq!(run.receipt().standing(), Standing::Clean);
    assert!(run.receipt().long_lines().is_empty());
}

#[test]
fn a_new_file_has_zero_per_file_debt() {
    let repository = fixture("long-lines-new-file");
    git_commit(&repository, "empty Rust baseline");
    repository.write("src/new.rs", &format!("{}\n", long("new")));
    git(&repository, &["add", "src/new.rs"]);

    let run = adjudicate(&repository);
    assert_eq!(run.receipt().standing(), Standing::Refused);
    let finding = &run.receipt().long_lines()[0];
    assert_eq!(finding.path(), "src/new.rs");
    assert_eq!((finding.baseline_debt(), finding.current_debt()), (0, 1));
}

#[test]
fn replacing_one_over_limit_line_fails_even_when_file_debt_holds() {
    let repository = fixture("long-lines-replacement");
    repository.write("src/lib.rs", &format!("{}\n", long("old")));
    git_commit(&repository, "one inherited long line");
    repository.write("src/lib.rs", &format!("{}\n", long("new")));

    let run = adjudicate(&repository);
    assert_eq!(run.receipt().standing(), Standing::Refused);
    let finding = &run.receipt().long_lines()[0];
    assert_eq!((finding.path(), finding.line()), ("src/lib.rs", 1));
    assert_eq!((finding.baseline_debt(), finding.current_debt()), (1, 1));
}

#[test]
fn a_long_line_only_refusal_names_its_actionable_locus_first() {
    let repository = fixture("long-line-refusal-detail");
    git_commit(&repository, "empty Rust baseline");
    repository.write(
        "src/detail.rs",
        &format!("short\n{}\n", "x".repeat(LONG_LINE_LIMIT + 1)),
    );
    git(&repository, &["add", "src/detail.rs"]);
    let contract = Contract::load(&repository.root).expect("load current contract");

    let run = contract
        .adjudicate(&repository.root)
        .expect("adjudicate long-line-only tree");
    assert_eq!(run.receipt().standing(), Standing::Refused);
    assert!(run.receipt().findings().is_empty());

    let detail = gate::report::refusal_detail(&repository.root, &contract, &run.report());
    assert!(detail.contains("src/detail.rs:2"), "{detail}");
    assert!(
        detail.contains(&format!("is {} bytes", LONG_LINE_LIMIT + 1)),
        "{detail}"
    );
    assert!(
        detail.contains(&format!("limit {LONG_LINE_LIMIT}")),
        "{detail}"
    );
}

fn fixture(label: &str) -> Tree {
    let repository = git_repository(label);
    repository.write("spice-rust.toml", gate::NARROWEST_FILE_CLASSES);
    repository.write("baseline.toml", BASELINE);
    repository
}

fn adjudicate(repository: &Tree) -> gate::CurrentRun {
    Contract::load(&repository.root)
        .expect("load current contract")
        .adjudicate(&repository.root)
        .expect("adjudicate current contract against captured HEAD")
}

fn long(label: &str) -> String {
    format!("{label}{}", "x".repeat(LONG_LINE_LIMIT + 1))
}
