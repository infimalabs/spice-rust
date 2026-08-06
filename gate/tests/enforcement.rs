//! The enforcement floor, exercised against real trees and real processes.
//!
//! Every fixture here builds a foreign repository on disk and runs the gate at
//! it, because the properties under test are about what a walk finds and what a
//! parser prints, and neither of those can be proven by a hand-built value.

use std::fs;
use std::time::Duration;

use gate::jurisdiction::{Admission, Shape};
use gate::measure::{Bounds, MeasureError, Measurement};
use gate::ratchet::{Malady, Movement};
use gate::{
    Contract, Ignore, Jurisdiction, Metric, Parser, Policy, Ratchet, Run, RunError, Standing,
    Verdict, measurement_digest,
};
use sill::Deadlines;
use wire::{Digest, Domain};

mod support;

use support::{Tree, fixture_git_barrier, git_commit, git_repository};

/// A line counter written entirely in shell builtins.
///
/// The parser runs with a cleared environment and therefore with no `PATH` to
/// find anything by, so a fixture that shelled out to `wc` would be testing the
/// host's layout rather than the gate. Builtins have no such dependency.
///
/// It is one line because a policy renders one declaration per line, and the
/// gate refuses a declared value that could open a line of its own.
const COUNTER: &str = "count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf '%s\\n' \"$count\"";

/// Count the selected state, then alter one selected file before returning it.
const MUTATING_COUNTER: &str = "count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf 'added by parser\\n' >> \"$1\"; printf '%s\\n' \"$count\"";

/// Change the selected file, count the transient bytes, and restore it before
/// returning a successful count.
const RESTORING_COUNTER: &str = "printf 'one\\ntwo\\n' > \"$1\"; count=0; \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$1\"; \
     printf 'one\\n' > \"$1\"; printf '%s\\n' \"$count\"";

/// Make the same transient change but return a value no metric can read.
const RESTORING_UNREADABLE_COUNTER: &str = "printf 'one\\ntwo\\n' > \"$1\"; \
     printf 'one\\n' > \"$1\"; printf 'not-a-count\\n'";

/// Mutate the live root named by the first fixed argument, then count only the
/// captured file arguments that follow it.
const MUTATING_LIVE_COUNTER: &str = "printf 'changed\\n' >> \"$1/src/lib.rs\"; \
     printf 'added\\n' > \"$1/added.rs\"; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf '%s\\n' \"$count\"";

/// Create and append the same untracked browser log while the parser process
/// owns the measurement boundary, then count only captured source arguments.
const MUTATING_UNTRACKED_COUNTER: &str = "root=$1; shift; \
     printf 'opened\\n' > \"$root/.playwright-mcp/console-123.log\"; \
     printf 'closed\\n' >> \"$root/.playwright-mcp/console-123.log\"; \
     count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf '%s\\n' \"$count\"";

/// Add a new live path to the Git index after the captured source was counted.
const ADD_TRACKED_LIVE_AFTER_COUNT: &str = "root=$1; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf 'added\\n' > \"$root/added.rs\"; \
     /usr/bin/git -C \"$root\" add added.rs; printf '%s\\n' \"$count\"";

/// Delete one live tracked path after the captured source was counted.
const DELETE_TRACKED_LIVE_AFTER_COUNT: &str = "root=$1; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; /bin/rm \"$root/src/lib.rs\"; printf '%s\\n' \"$count\"";

/// Replace one live tracked file with a directory after counting its snapshot.
const RESHAPE_TRACKED_LIVE_AFTER_COUNT: &str = "root=$1; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; /bin/rm \"$root/src/lib.rs\"; /bin/mkdir \"$root/src/lib.rs\"; \
     printf '%s\\n' \"$count\"";

const MUTATE_LIVE_AFTER_COUNT: &str = "root=$1; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf 'one\\ntwo\\nthree\\n' > \"$root/src/lib.rs\"; printf '%s\\n' \"$count\"";

const RESTORE_LIVE_AFTER_COUNT: &str = "root=$1; shift; count=0; for file in \"$@\"; do \
     while IFS= read -r line || [ -n \"$line\" ]; do count=$((count + 1)); done < \"$file\"; \
     done; printf 'one\\ntwo\\n' > \"$root/src/lib.rs\"; printf '%s\\n' \"$count\"";

fn bounds() -> Bounds {
    bounds_with(64 * 1024, Duration::from_secs(5))
}

fn bounds_with(capture: usize, output_drain: Duration) -> Bounds {
    Bounds::new(
        "gate-fixture",
        Deadlines::new(
            Duration::from_secs(30),
            output_drain,
            Duration::from_secs(5),
        )
        .expect("declare deadlines"),
        Duration::from_millis(100),
        capture,
    )
    .expect("declare bounds")
}

fn line_metric() -> Metric {
    Metric::new(
        "rust.lines",
        "rust",
        "total lines in admitted rust sources",
        Parser::new("/bin/sh", ["-c", COUNTER, "sh"]).expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric")
}

fn live_root_metric(name: &str, target: &str, script: &str, root: &std::path::Path) -> Metric {
    Metric::new(
        name,
        "rust",
        target,
        Parser::new(
            "/bin/sh",
            [
                "-c".to_owned(),
                script.to_owned(),
                "sh".to_owned(),
                root.to_string_lossy().into_owned(),
            ],
        )
        .expect("declare a live-root parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a live-root metric")
}

fn policy(ignore: &str) -> Policy {
    Policy::new(
        Ignore::parse(ignore).expect("parse the fixture ignore policy"),
        [line_metric()],
    )
    .expect("declare a policy")
}

fn ratchet(text: &str) -> Ratchet {
    Ratchet::parse(text).expect("parse the fixture ratchet")
}

#[test]
fn a_walk_records_a_reason_for_every_path_it_reaches() {
    let tree = Tree::new("reasons");
    tree.write("src/lib.rs", "one\ntwo\n");
    tree.write("target/junk.rs", "noise\n");
    tree.write("notes.txt", "prose\n");

    let ignore = Ignore::parse("/target/\n*.txt\n").expect("parse the policy");
    let walked = Jurisdiction::walk(&tree.root, &ignore).expect("walk the fixture");

    let recorded: Vec<(&str, Shape, Admission)> = walked
        .entries()
        .iter()
        .map(|entry| (entry.path(), entry.shape(), entry.admission()))
        .collect();

    // `target` is recorded as excluded and never descended, so the absence of
    // `target/junk.rs` is explained by the record rather than merely true.
    assert_eq!(
        recorded,
        [
            ("notes.txt", Shape::File, Admission::Ignored(1)),
            ("src", Shape::Directory, Admission::Unnamed),
            ("src/lib.rs", Shape::File, Admission::Unnamed),
            ("target", Shape::Directory, Admission::Ignored(0)),
        ]
    );
    assert_eq!(
        walked.inventory(),
        "ignored:1 file notes.txt\n\
         unnamed directory src\n\
         unnamed file src/lib.rs\n\
         ignored:0 directory target\n"
    );
}

#[test]
fn a_readmitting_rule_beats_the_broad_rule_above_it() {
    let tree = Tree::new("readmit");
    tree.write("vendor/keep.rs", "kept\n");
    tree.write("vendor/drop.rs", "dropped\n");

    let ignore = Ignore::parse("vendor/*.rs\n!vendor/keep.rs\n").expect("parse the policy");
    let walked = Jurisdiction::walk(&tree.root, &ignore).expect("walk the fixture");

    let admitted: Vec<&str> = walked.measurable().map(gate::Entry::path).collect();
    assert_eq!(admitted, ["vendor/keep.rs"]);
    assert_eq!(
        walked
            .entries()
            .iter()
            .find(|entry| entry.path() == "vendor/keep.rs")
            .map(gate::Entry::admission),
        Some(Admission::Readmitted(1)),
        "the later rule is the one that decided, and the record says so"
    );
}

#[test]
fn a_walk_is_a_total_function_of_the_tree() {
    let tree = Tree::new("total");
    for index in 0..24 {
        tree.write(&format!("crate{index}/src/lib.rs"), "line\n");
    }

    let ignore = Ignore::parse("").expect("parse an empty policy");
    let first = Jurisdiction::walk(&tree.root, &ignore).expect("walk once");
    let again = Jurisdiction::walk(&tree.root, &ignore).expect("walk again");

    assert_eq!(first.inventory(), again.inventory());
    assert_eq!(first.inventory_digest(), again.inventory_digest());
    assert_eq!(
        first.tree_digest().expect("digest the tree"),
        again.tree_digest().expect("digest the tree again")
    );

    let paths: Vec<&str> = first.entries().iter().map(gate::Entry::path).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted, "entries are sorted, not merely stable");
}

/// Gate publishes five digest classes, including the file digest nested inside
/// the tree listing. Freeze every spelling so moving one mint site to another
/// domain cannot silently change what an existing receipt identity means.
#[test]
fn every_gate_digest_is_frozen_at_its_published_spelling() {
    let tree = Tree::new("digest-domains");
    let file_bytes = b"one\ntwo\n";
    tree.write(
        "fixture.txt",
        std::str::from_utf8(file_bytes).expect("fixture is text"),
    );
    let jurisdiction = Jurisdiction::walk(
        &tree.root,
        &Ignore::parse("").expect("parse an empty policy"),
    )
    .expect("walk the fixture");

    // `tree_digest` mints the file identity internally. Reconstructing its
    // one-entry listing with the named file domain ties that nested mint site
    // to this fixture without exposing a file-digest API solely for a test.
    let file_digest = Digest::compute(Domain::GateFile, file_bytes);
    let tree_listing = format!("{file_digest} fixture.txt\n");
    let tree_digest = jurisdiction.tree_digest().expect("digest the tree");
    assert_eq!(
        tree_digest,
        Digest::compute(Domain::GateTree, tree_listing.as_bytes()),
        "the tree must nest GateFile identities under GateTree",
    );

    let policy = Policy::new(
        Ignore::parse("*.tmp\n").expect("parse the fixture policy"),
        Vec::<Metric>::new(),
    )
    .expect("declare the fixture policy");
    let measurements = [Measurement::new("fixture.lines", 2, ["fixture.txt"])];

    for (class, computed, frozen) in [
        (
            Domain::GateFile,
            file_digest,
            "2edc805176833b058cd6136c912ff6c9e3b7c430d158e7172b0e56a6bec84f2c",
        ),
        (
            Domain::GateTree,
            tree_digest,
            "73c42b39bc0fcbdeb3f3e38250edb0b703aa168b353726e224bbc91254c73f2e",
        ),
        (
            Domain::GateInventory,
            jurisdiction.inventory_digest(),
            "f01b9548a1181e65ac91efa6bf4489560340c29e075b3db3ca79e741a5be0171",
        ),
        (
            Domain::GatePolicy,
            policy.digest(),
            "18a0e223485c527ffdbe4332a5a49790c4eff66ed1deb536acaa665d82265cdc",
        ),
        (
            Domain::GateMeasurement,
            measurement_digest(&measurements),
            "61d093ca25f57ff332ea5154d018806347b75dfec952b70a3153f9cd1d3496d9",
        ),
    ] {
        assert_eq!(
            computed.as_str(),
            frozen,
            "{} no longer digests to the spelling it published",
            class.tag(),
        );
    }
}

#[test]
fn ignore_lines_with_more_than_one_spelling_are_refused() {
    for line in [" *.rs", "*.rs ", "!"] {
        let refused = Ignore::parse(line).expect_err("a second spelling is refused");
        assert_eq!(refused.line(), line, "the refusal quotes the exact line");
    }

    let accepted = Ignore::parse("# a comment\n\n**/*.rs\na\\ b\n")
        .expect("comments, blanks, crossing stars, and escapes are Gitignore syntax");
    assert_eq!(accepted.rules().len(), 2);
    assert_eq!(accepted.render(), "**/*.rs\na\\ b\n");

    // A rendered policy has to parse back to itself, or the policy digest would
    // be over a spelling that no operator could commit and re-derive.
    let written = "*.rs\n/target/\nvendor/*.rs\n!vendor/keep.rs\na b\n";
    let parsed = Ignore::parse(written).expect("parse a policy with every rule shape");
    assert_eq!(parsed.render(), written);
    assert_eq!(
        Ignore::parse(&parsed.render()).expect("reparse the rendering"),
        parsed
    );
}

#[test]
fn a_clean_run_earns_a_tighter_baseline_and_a_regression_earns_nothing() {
    let tree = Tree::new("clean");
    tree.write("src/lib.rs", "one\ntwo\nthree\n");

    let policy = policy("/target/\n");
    let held = ratchet("rust.lines 3\n");
    let run = Run::adjudicate(&policy, &tree.root, &held, &bounds()).expect("adjudicate");

    assert_eq!(run.standing(), Standing::Clean);
    assert_eq!(run.measurements()[0].value(), 3);
    assert_eq!(run.findings()[0].movement, Movement::Held { baseline: 3 });
    assert_eq!(
        run.earned(&held)
            .expect("a clean run earns a ratchet")
            .render(),
        "rust.lines 3\n"
    );

    tree.write("src/lib.rs", "one\ntwo\nthree\nfour\n");
    let regressed = Run::adjudicate(&policy, &tree.root, &held, &bounds()).expect("adjudicate");

    assert_eq!(regressed.standing(), Standing::Refused);
    assert_eq!(
        regressed.findings()[0].movement,
        Movement::Regressed {
            baseline: 3,
            measured: 4
        }
    );
    assert!(
        regressed.earned(&held).is_none(),
        "a refused run must not hand a lower baseline to a tree that failed"
    );
}

#[test]
fn an_improvement_lowers_the_baseline_and_no_run_ever_raises_one() {
    let tree = Tree::new("improved");
    tree.write("src/lib.rs", "one\n");

    let held = ratchet("rust.lines 9\n");
    let run = Run::adjudicate(&policy(""), &tree.root, &held, &bounds()).expect("adjudicate");

    assert_eq!(run.standing(), Standing::Clean);
    assert_eq!(
        run.findings()[0].movement,
        Movement::Shrank {
            baseline: 9,
            measured: 1
        }
    );
    assert_eq!(
        run.earned(&held).expect("an improvement earns").render(),
        "rust.lines 1\n"
    );

    // The shrink-only promise is about every reachable pairing, not about the
    // one this fixture happened to walk into. Both sides are varied, because a
    // sweep that holds the measurement still would pass just as happily against
    // a `tighten` that took whatever it was handed.
    for baseline in 0..12_u64 {
        let committed = ratchet(&format!("rust.lines {baseline}\n"));
        for measured in 0..12_u64 {
            let taken = [Measurement::new("rust.lines", measured, ["src/lib.rs"])];
            let earned = committed.tighten(&taken).baseline("rust.lines");
            assert_eq!(
                earned,
                Some(baseline.min(measured)),
                "tightening {baseline} against {measured} did not take the lower"
            );
        }
    }
}

#[test]
fn a_baseline_whose_metric_stopped_being_declared_refuses_rather_than_evaporating() {
    let tree = Tree::new("retired");
    tree.write("src/lib.rs", "one\n");

    let held = ratchet("rust.lines 4\nrust.unsafe 0\n");
    let run = Run::adjudicate(&policy(""), &tree.root, &held, &bounds()).expect("adjudicate");

    assert_eq!(
        run.standing(),
        Standing::Refused,
        "a baseline nothing measures has risen to infinity in all but spelling"
    );
    assert_eq!(
        run.findings()[1].movement,
        Movement::Retired { baseline: 0 }
    );
    assert!(run.earned(&held).is_none());
}

#[test]
fn a_malformed_baseline_is_refused_by_line_and_never_half_read() {
    for (text, number, malady) in [
        ("rust.lines\n", 1, Malady::NotTwoFields),
        ("rust.lines  4\n", 1, Malady::NotACount),
        ("rust.lines -1\n", 1, Malady::NotACount),
        ("a 1\na 2\n", 2, Malady::Duplicate),
        ("b 1\na 2\n", 2, Malady::Unsorted),
    ] {
        let refused = Ratchet::parse(text).expect_err("a malformed ratchet is refused");
        assert_eq!((refused.number(), refused.malady()), (number, malady));
    }

    let read = ratchet("a 1\nb 2\n");
    assert_eq!(read.render(), "a 1\nb 2\n", "one baseline, one spelling");
}

#[test]
fn completed_run_identity_moves_when_any_observed_input_moves() {
    let tree = Tree::new("receipt");
    tree.write("src/lib.rs", "one\ntwo\n");

    let held = ratchet("rust.lines 2\n");
    let first = Run::adjudicate(&policy(""), &tree.root, &held, &bounds()).expect("adjudicate");
    let original_tree = first.tree().clone();
    let original_measurement = first.measurement().clone();
    let original_inventory = first.inventory().clone();

    // Same line count, different bytes: the tree moved and nothing else did.
    tree.write("src/lib.rs", "ONE\nTWO\n");
    let rewritten = Run::adjudicate(&policy(""), &tree.root, &held, &bounds()).expect("adjudicate");
    assert_ne!(rewritten.tree(), &original_tree);
    assert_eq!(rewritten.measurement(), &original_measurement);
    assert_eq!(rewritten.inventory(), &original_inventory);

    // A file nobody measures still moves the inventory, because the record of
    // what was refused is part of what the receipt stands behind.
    tree.write("notes.txt", "prose\n");
    let noticed =
        Run::adjudicate(&policy("*.txt\n"), &tree.root, &held, &bounds()).expect("adjudicate");
    assert_ne!(noticed.inventory(), rewritten.inventory());
    assert_eq!(noticed.tree(), rewritten.tree());
    assert_ne!(
        noticed.policy(),
        rewritten.policy(),
        "the ignore rules are part of the declaration the receipt names"
    );

    // And the fourth: a different count is a different measurement, so the last
    // of the four digests is pinned to something that can move on its own too.
    tree.write("src/lib.rs", "ONE\nTWO\nTHREE\n");
    let counted =
        Run::adjudicate(&policy("*.txt\n"), &tree.root, &held, &bounds()).expect("adjudicate");
    assert_ne!(counted.measurement(), noticed.measurement());
    assert_eq!(counted.inventory(), noticed.inventory());
    assert_eq!(counted.policy(), noticed.policy());
}

#[test]
fn a_published_receipt_survives_a_round_trip_and_an_interrupted_successor() {
    let tree = git_repository("publish");
    tree.write("src/lib.rs", "one\n");
    let receipt = tree.path("verdict.json");

    let policy_path = tree.path("spice.toml");
    let baseline_path = tree.path("baseline.toml");
    let baseline = "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n";
    tree.write("spice.toml", gate::NARROWEST_FILE_CLASSES);
    tree.write("baseline.toml", baseline);
    git_commit(&tree, "receipt fixture");
    let contract = Contract::from_tracked(
        &tree.root,
        &policy_path,
        gate::NARROWEST_FILE_CLASSES,
        &baseline_path,
        baseline,
    )
    .expect("load current contract");
    let run = contract
        .adjudicate(&tree.root)
        .expect("adjudicate current contract");
    assert_eq!(run.receipt().tree(), run.run().tree());
    assert_eq!(run.receipt().gate_set(), &contract.gate_set().digest());
    assert_eq!(run.receipt().policy(), &contract.policy().digest());
    assert_eq!(run.receipt().inventory(), run.run().inventory());
    assert_eq!(run.receipt().measurement(), run.run().measurement());
    assert_eq!(run.receipt().bounds(), &contract.bounds().digest());
    assert_eq!(run.receipt().baseline(), &contract.baseline().digest());
    assert_eq!(run.receipt().tools().len(), contract.tools().len());
    assert_eq!(
        run.receipt().tools()[0].identity(),
        contract.tools()[0].digest()
    );
    run.receipt()
        .publish(&receipt)
        .expect("publish the receipt");

    let published = fs::read(&receipt).expect("read the published receipt");
    let retained = String::from_utf8(published.clone()).expect("receipt JSON is UTF-8");
    assert!(!retained.contains("/usr/bin/awk"));
    assert!(!retained.contains("length($0)"));
    assert!(!retained.contains(".git/"));
    assert!(!retained.contains("runtime-ms"));
    let decoded = Verdict::decode(&published).expect("decode the published receipt");
    assert_eq!(&decoded, run.receipt());

    // A run that dies between staging and the swap leaves the previous receipt
    // exactly as it was, and leaves no half-written file beside it either.
    let destination = atomic::Destination::new(&receipt).expect("resolve the destination");
    let staged = destination.stage().expect("stage a successor");
    let temporary = staged.temporary_path().to_path_buf();
    drop(staged);

    assert!(!temporary.exists(), "an abandoned stage leaves no residue");
    assert_eq!(
        fs::read(&receipt).expect("read the receipt again"),
        published,
        "an interrupted publication left the previous receipt intact"
    );
}

#[test]
fn a_parser_that_outruns_its_deadline_yields_no_number() {
    let tree = Tree::new("bounds");
    tree.write("src/lib.rs", "one\n");

    let runaway = Metric::new(
        "rust.lines",
        "rust",
        "a parser that never finishes",
        Parser::new("/bin/sh", ["-c", "while :; do :; done"]).expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let bounded = Bounds::new(
        "gate-fixture",
        Deadlines::new(
            Duration::from_millis(150),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .expect("declare deadlines"),
        Duration::from_millis(50),
        1024,
    )
    .expect("declare bounds");

    let ignore = Ignore::parse("").expect("parse an empty policy");
    let walked = Jurisdiction::walk(&tree.root, &ignore).expect("walk the fixture");
    let refused = runaway
        .measure(&walked, &bounded)
        .expect_err("a killed parser produces no measurement");

    // The distinction that matters is between "measured zero" and "did not
    // measure", because only the first may ever reach a ratchet.
    assert!(
        matches!(refused, MeasureError::Unreadable { .. }),
        "expected no number at all, got {refused:?}"
    );
}

#[test]
fn a_numeric_prefix_from_truncated_stdout_yields_no_number() {
    let tree = Tree::new("truncated");
    tree.write("src/lib.rs", "one\n");
    let metric = Metric::new(
        "rust.lines",
        "rust",
        "a count whose retained prefix is still numeric",
        Parser::new("/bin/sh", ["-c", "printf '1junk'", "sh"]).expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let walked = Jurisdiction::walk(&tree.root, &Ignore::parse("").expect("parse ignore"))
        .expect("walk the fixture");

    let refused = metric
        .measure(&walked, &bounds_with(1, Duration::from_secs(5)))
        .expect_err("a truncated count is not a measurement");

    assert!(
        matches!(
            &refused,
            MeasureError::Unreadable { evidence, .. }
                if evidence == "truncated stdout capture: 1"
        ),
        "the retained numeric prefix was not refused by capture state: {refused:?}",
    );
}

#[test]
fn a_numeric_prefix_from_incomplete_stdout_yields_no_number() {
    let tree = Tree::new("incomplete");
    tree.write("src/lib.rs", "one\n");
    let metric = Metric::new(
        "rust.lines",
        "rust",
        "a count whose output pipe remains open",
        Parser::new("/bin/sh", ["-c", "/bin/sleep 1 & printf '1'", "sh"])
            .expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let walked = Jurisdiction::walk(&tree.root, &Ignore::parse("").expect("parse ignore"))
        .expect("walk the fixture");

    let refused = metric
        .measure(&walked, &bounds_with(1024, Duration::from_millis(10)))
        .expect_err("an incompletely drained count is not a measurement");

    assert!(
        matches!(
            &refused,
            MeasureError::Unreadable { evidence, .. }
                if evidence == "incomplete stdout capture: 1"
        ),
        "the retained numeric prefix was not refused by drain state: {refused:?}",
    );
}

#[test]
fn a_parser_cannot_move_the_tree_behind_its_measurement() {
    let tree = Tree::new("mutating-parser");
    tree.write("src/lib.rs", "one\n");
    let mutating_metric = Metric::new(
        "rust.lines",
        "rust",
        "lines before the parser mutates its input",
        Parser::new("/bin/sh", ["-c", MUTATING_COUNTER, "sh"]).expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let policy = Policy::new(Ignore::parse("").expect("parse ignore"), [mutating_metric])
        .expect("declare a policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("a run over two tree states cannot produce a receipt");

    assert!(
        matches!(
            &refused,
            RunError::SnapshotChanged {
                metric,
                measured: Some(1),
                path,
                tree,
                observed_tree: Some(observed_tree),
            } if metric == "rust.lines" && path == "src/lib.rs" && tree != observed_tree
        ),
        "the mutating parser was not refused by the snapshot seal: {refused:?}",
    );
    assert_eq!(
        fs::read_to_string(tree.path("src/lib.rs")).expect("read the mutated source"),
        "one\n",
        "a parser receives the private snapshot rather than the live source",
    );
}

#[test]
fn a_parser_cannot_measure_transient_bytes_then_restore_them() {
    let tree = Tree::new("restoring-parser");
    tree.write("src/lib.rs", "one\n");
    let restoring_metric = Metric::new(
        "rust.lines",
        "rust",
        "lines while the parser rewrites its input",
        Parser::new("/bin/sh", ["-c", RESTORING_COUNTER, "sh"]).expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let policy = Policy::new(Ignore::parse("").expect("parse ignore"), [restoring_metric])
        .expect("declare a policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 2\n"), &bounds())
        .expect_err("a transient measurement cannot produce a verdict");

    assert!(
        matches!(
            &refused,
            RunError::SnapshotChanged {
                metric,
                measured: Some(2) | None,
                path,
                tree,
                observed_tree: Some(observed_tree),
            } if metric == "rust.lines" && path == "src/lib.rs" && tree == observed_tree
        ),
        "the restored bytes hid the transient measurement: {refused:?}",
    );
    assert_eq!(
        fs::read_to_string(tree.path("src/lib.rs")).expect("read the live source"),
        "one\n",
    );
}

/// Snapshot mutation outranks a parser result that cannot become a measurement.
///
/// Adjudication verifies the private snapshot before it propagates the parser's
/// error. The transient write therefore remains the refusal even when the
/// parser returned no trustworthy count; restored bytes make the two tree
/// digests equal without erasing the fingerprint or change-notification seal.
#[test]
fn restored_snapshot_change_outranks_an_unreadable_measurement() {
    let tree = Tree::new("restoring-unreadable-parser");
    tree.write("src/lib.rs", "one\n");
    let restoring_metric = Metric::new(
        "rust.lines",
        "rust",
        "lines while an unreadable parser rewrites its input",
        Parser::new("/bin/sh", ["-c", RESTORING_UNREADABLE_COUNTER, "sh"])
            .expect("declare a parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let policy = Policy::new(Ignore::parse("").expect("parse ignore"), [restoring_metric])
        .expect("declare a policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 2\n"), &bounds())
        .expect_err("snapshot mutation must outrank an unreadable count");

    assert!(
        matches!(
            &refused,
            RunError::SnapshotChanged {
                metric,
                measured: None,
                path,
                tree,
                observed_tree: Some(observed_tree),
            } if metric == "rust.lines" && path == "src/lib.rs" && tree == observed_tree
        ),
        "restored bytes erased the mutation when measurement also refused: {refused:?}",
    );
    assert_eq!(
        fs::read_to_string(tree.path("src/lib.rs")).expect("read the live source"),
        "one\n",
    );
}

#[test]
fn persistent_live_tree_and_inventory_mutation_remains_named() {
    let tree = Tree::new("live-mutation");
    tree.write("src/lib.rs", "one\n");
    let parser = Parser::new(
        "/bin/sh",
        [
            "-c".to_owned(),
            MUTATING_LIVE_COUNTER.to_owned(),
            "sh".to_owned(),
            tree.root.to_string_lossy().into_owned(),
        ],
    )
    .expect("declare a parser");
    let metric = Metric::new(
        "rust.lines",
        "rust",
        "lines before the live tree changes",
        parser,
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let policy =
        Policy::new(Ignore::parse("").expect("parse ignore"), [metric]).expect("declare a policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("persistent live mutation refuses");

    assert!(
        matches!(
            &refused,
            RunError::JurisdictionChanged {
                paths,
                inventory,
                observed_inventory,
                tree,
                observed_tree,
            } if paths == &["added.rs", "src/lib.rs"]
                && inventory != observed_inventory
                && tree != observed_tree
        ),
        "the live tree and inventory mutation lost its named refusal: {refused:?}",
    );
    let rendered = refused.to_string();
    assert!(rendered.contains("added.rs"), "{rendered}");
    assert!(rendered.contains("src/lib.rs"), "{rendered}");
}

#[test]
fn tracked_index_jurisdiction_excludes_scratch_created_during_measurement() {
    let tree = git_repository("tracked-scratch-mutation");
    tree.write("src/lib.rs", "one\n");
    git_commit(&tree, "tracked source");
    fs::create_dir(tree.path(".playwright-mcp")).expect("create browser scratch directory");
    let metric = || {
        live_root_metric(
            "rust.lines",
            "lines while an untracked browser log changes",
            MUTATING_UNTRACKED_COUNTER,
            &tree.root,
        )
    };
    let ignore = Ignore::parse(".git/\n").expect("parse Git metadata exclusion");

    let whole = Policy::new(ignore.clone(), [metric()]).expect("declare whole-walk policy");
    let refused = Run::adjudicate(&whole, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("a whole-tree walk observes the changing scratch file");
    assert!(
        matches!(
            &refused,
            RunError::JurisdictionChanged { paths, .. }
                if paths == &[".playwright-mcp/console-123.log"]
        ),
        "the control walk did not name the mutable browser log: {refused:?}",
    );
    fs::remove_file(tree.path(".playwright-mcp/console-123.log"))
        .expect("reset browser scratch file");

    let tracked = Policy::tracked_index(ignore, [metric()]).expect("declare tracked-index policy");
    let run = Run::adjudicate(&tracked, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect("untracked browser scratch cannot move tracked jurisdiction");
    assert_eq!(run.standing(), Standing::Clean);
    assert!(
        run.jurisdiction()
            .entries()
            .iter()
            .all(|entry| entry.path() != ".playwright-mcp/console-123.log")
    );
    assert_eq!(
        fs::read_to_string(tree.path(".playwright-mcp/console-123.log"))
            .expect("read browser scratch evidence"),
        "opened\nclosed\n"
    );
}

#[test]
fn tracked_index_jurisdiction_names_current_content_drift() {
    let tree = git_repository("tracked-content-mutation");
    tree.write("src/lib.rs", "one\n");
    git_commit(&tree, "tracked source");
    let metric = live_root_metric(
        "rust.lines",
        "lines before tracked current contents change",
        MUTATE_LIVE_AFTER_COUNT,
        &tree.root,
    );
    let policy = Policy::tracked_index(Ignore::parse("").expect("parse ignore"), [metric])
        .expect("declare tracked-index policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("tracked current content drift refuses");
    assert!(
        matches!(
            &refused,
            RunError::JurisdictionChanged { paths, .. } if paths == &["src/lib.rs"]
        ),
        "tracked content drift lost its path: {refused:?}",
    );
    assert!(refused.to_string().contains("src/lib.rs"));
}

#[test]
fn tracked_index_jurisdiction_names_index_path_creation() {
    let tree = git_repository("tracked-path-creation");
    tree.write("src/lib.rs", "one\n");
    git_commit(&tree, "tracked source");
    let metric = live_root_metric(
        "rust.lines",
        "lines before the tracked path set grows",
        ADD_TRACKED_LIVE_AFTER_COUNT,
        &tree.root,
    );
    let policy = Policy::tracked_index(Ignore::parse("").expect("parse ignore"), [metric])
        .expect("declare tracked-index policy");

    let _git_barrier = fixture_git_barrier();
    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("tracked path creation refuses");
    assert!(
        matches!(
            &refused,
            RunError::JurisdictionChanged { paths, .. } if paths == &["added.rs"]
        ),
        "tracked path creation lost its path: {refused:?}",
    );
    assert!(refused.to_string().contains("added.rs"));
}

#[test]
fn tracked_index_jurisdiction_names_shape_drift() {
    let tree = git_repository("tracked-shape-mutation");
    tree.write("src/lib.rs", "one\n");
    git_commit(&tree, "tracked source");
    let metric = live_root_metric(
        "rust.lines",
        "lines before a tracked file changes shape",
        RESHAPE_TRACKED_LIVE_AFTER_COUNT,
        &tree.root,
    );
    let policy = Policy::tracked_index(Ignore::parse("").expect("parse ignore"), [metric])
        .expect("declare tracked-index policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("tracked shape drift refuses");
    assert!(
        matches!(
            &refused,
            RunError::JurisdictionChanged { paths, .. } if paths == &["src/lib.rs"]
        ),
        "tracked shape drift lost its path: {refused:?}",
    );
    assert!(refused.to_string().contains("src/lib.rs"));
}

#[test]
fn tracked_index_jurisdiction_refuses_a_deleted_path_by_name() {
    let tree = git_repository("tracked-deletion");
    tree.write("src/lib.rs", "one\n");
    git_commit(&tree, "tracked source");
    let metric = live_root_metric(
        "rust.lines",
        "lines before a tracked path disappears",
        DELETE_TRACKED_LIVE_AFTER_COUNT,
        &tree.root,
    );
    let policy = Policy::tracked_index(Ignore::parse("").expect("parse ignore"), [metric])
        .expect("declare tracked-index policy");

    let refused = Run::adjudicate(&policy, &tree.root, &ratchet("rust.lines 1\n"), &bounds())
        .expect_err("tracked deletion refuses");
    assert!(matches!(&refused, RunError::Walk(_)), "{refused:?}");
    assert!(refused.to_string().contains("src/lib.rs"));
}

#[test]
fn every_metric_reads_the_same_captured_jurisdiction() {
    let tree = Tree::new("multiple-metrics");
    tree.write("src/lib.rs", "one\ntwo\n");
    let root = tree.root.to_string_lossy().into_owned();
    let metric = |name: &str, script: &str| {
        Metric::new(
            name,
            "rust",
            "lines in the captured source",
            Parser::new(
                "/bin/sh",
                [
                    "-c".to_owned(),
                    script.to_owned(),
                    "sh".to_owned(),
                    root.clone(),
                ],
            )
            .expect("declare a parser"),
            "*.rs\n",
            "",
        )
        .expect("declare a metric")
    };
    let policy = Policy::new(
        Ignore::parse("").expect("parse ignore"),
        [
            metric("rust.first", MUTATE_LIVE_AFTER_COUNT),
            metric("rust.second", RESTORE_LIVE_AFTER_COUNT),
        ],
    )
    .expect("declare a policy");

    let run = Run::adjudicate(
        &policy,
        &tree.root,
        &ratchet("rust.first 2\nrust.second 2\n"),
        &bounds(),
    )
    .expect("both metrics measure one snapshot");

    assert_eq!(
        run.measurements()
            .iter()
            .map(|measurement| (
                measurement.metric(),
                measurement.value(),
                measurement.files()
            ))
            .collect::<Vec<_>>(),
        [
            ("rust.first", 2, ["src/lib.rs".to_owned()].as_slice()),
            ("rust.second", 2, ["src/lib.rs".to_owned()].as_slice()),
        ]
    );
    assert_eq!(
        run.tree(),
        &run.jurisdiction().tree_digest().expect("digest live tree"),
    );
    assert_eq!(
        fs::read_to_string(tree.path("src/lib.rs")).expect("read restored live tree"),
        "one\ntwo\n",
    );
}

#[test]
fn a_metric_that_omits_what_doctrine_requires_is_refused() {
    let parser = || Parser::new("/bin/sh", ["-c", COUNTER, "sh"]).expect("declare a parser");
    let declare = |name: &str, language: &str, target: &str, files: &str, exclusions: &str| {
        Metric::new(name, language, target, parser(), files, exclusions)
    };

    for (name, language, target, files) in [
        ("", "rust", "lines", "*.rs\n"),
        ("rust.lines", "", "lines", "*.rs\n"),
        ("rust.lines", "rust", "", "*.rs\n"),
        ("rust.lines", "rust", "lines", ""),
    ] {
        assert!(
            declare(name, language, target, files, "").is_err(),
            "a metric missing a declaration was accepted"
        );
    }

    assert!(
        declare("rust lines", "rust", "lines", "*.rs\n", "").is_err(),
        "a spaced name would make a rendered policy line unsplittable"
    );
    assert!(
        declare("rust.lines", "rust", "lines", "!*.rs\n", "").is_err(),
        "scope is stated by the two lists, not by negation inside one of them"
    );
    assert!(Parser::new("", ["-c"]).is_err(), "a parser must be named");
}

#[test]
fn a_policy_renders_every_declaration_doctrine_names() {
    let rendered = policy("/target/\n").render();

    // Doctrine requires a metric to declare its files, language, parser,
    // target, and exclusions. Each has to be legible in the rendering, because
    // the rendering is the thing the policy digest actually covers.
    assert_eq!(
        rendered,
        format!(
            "ignore /target/\n\
             metric rust.lines language rust\n\
             metric rust.lines target total lines in admitted rust sources\n\
             metric rust.lines program /bin/sh\n\
             metric rust.lines argument -c\n\
             metric rust.lines argument {COUNTER}\n\
             metric rust.lines argument sh\n\
             metric rust.lines files *.rs\n"
        )
    );

    assert!(
        Policy::new(policy("").ignore().clone(), [line_metric(), line_metric()]).is_err(),
        "two metrics under one name would make a ratchet line ambiguous"
    );
}

#[test]
fn no_declaration_can_forge_a_policy_line() {
    // Arguments render one per line rather than space-joined, so these two
    // parsers are two policies. Joined, both would have spelled `-c a b`.
    let split = |arguments: [&str; 3]| {
        Policy::new(
            Ignore::parse("").expect("parse an empty policy"),
            [Metric::new(
                "rust.lines",
                "rust",
                "lines",
                Parser::new("/bin/sh", arguments).expect("declare a parser"),
                "*.rs\n",
                "",
            )
            .expect("declare a metric")],
        )
        .expect("declare a policy")
        .digest()
    };
    assert_ne!(split(["-c", "a b", "x"]), split(["-c", "a", "b x"]));

    // And a value carrying a line break is refused outright, which is what
    // keeps the one-per-line rendering injective in the first place.
    assert!(
        Parser::new("/bin/sh", ["-c\nmetric rust.lines files *.md"]).is_err(),
        "an argument opened a policy line of its own"
    );
    for forged in ["ru\nst", "li\rnes"] {
        assert!(
            Metric::new(
                "rust.lines",
                forged,
                "lines",
                Parser::new("/bin/sh", ["-c"]).expect("declare a parser"),
                "*.rs\n",
                "",
            )
            .is_err(),
            "a declared value carried a line break"
        );
    }
}
