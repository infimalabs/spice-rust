//! The git network surface and the boundaries that write, both enumerated.
//!
//! Two invariants govern how this system is allowed to touch git. GIT-01: the
//! agent itself never pulls and never pushes, and its working tree is touched
//! at exactly three control-plane boundaries. Operator-authored release
//! publication is a separate, enumerated control plane: planning may inspect
//! the configured remote, while applying a digest-approved plan may publish
//! through the tracked release authority. GIT-06: activation is deliberately
//! not another boundary, because it is replayed after every compaction and
//! would land git work mid-session between two of a live agent's own commands.
//!
//! Neither invariant is expressible as a unit test of any one function: both
//! are statements about what the whole tree does *not* contain. So they are
//! enumerated here from the tracked set instead. Each test declares what it
//! expects to find and compares that against a fresh scan, so the way to add a
//! network call or a new git entry point is to say so in the allowlist, in a
//! diff a reviewer reads, rather than to add it quietly and stay green.
//!
//! The first four tests fall into two deliberately independent pairs. The two
//! network tests read subcommand literals and know nothing about module
//! structure; the boundary test reads public entry points and knows nothing
//! about subcommands. A new network call site reds the network pair alone. The
//! boundary test alone reds for a new public git entry point, for a launch-time
//! entry point the resident stops calling, and for a git reference appearing in
//! the activation surface. The fourth test polices neither surface: it asserts
//! that this battery remains a named workspace target for scoped execution.
//! The fifth rejects Git process construction across repository test sources.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// This file, carved out of the scan it performs.
///
/// The allowlist below has to spell every subcommand it polices, so scanning
/// this file would report its own vocabulary as call sites. The carve-out is
/// fail-safe rather than trusted: it is matched by path, so renaming or moving
/// this file stops the carve-out from matching, the file is scanned like any
/// other, and its literals red the test instead of disappearing from it.
const SELF_PATH: &str = "xtask/tests/git_boundaries.rs";

/// Inert absolute-Git spellings retained by test assertions and pin fixtures.
///
/// Counts make each exception closed: another spelling in one of these files
/// still has to be reviewed and declared. The policy test below separately
/// rejects executable constructors that name Git through `PATH`.
const INERT_ABSOLUTE_GIT_TEST_SITES: [(&str, usize); 9] = [
    ("proof/src/bin/spice-proof-darwin-runner/tests.rs", 1),
    ("spice/src/cargo_isolation/tests.rs", 1),
    // Canvas proves an overlay entered its synthetic private index by querying
    // that disposable repository directly.
    ("spice/src/gate_command/prove/canvas/workspace.rs", 1),
    // These fixture helpers create and stage only their own temporary
    // repositories; absolute resolution keeps the tests independent of PATH.
    ("spice/src/gate_command/prove/tests.rs", 1),
    ("spice/tests/gate_routing_admission.rs", 1),
    ("spice/tests/gate_single_flight/fixture.rs", 1),
    // The instrumented rustfmt script deliberately moves its fixture index to
    // prove a concurrent mutation is detected before evidence can publish.
    ("spice/tests/gate_single_flight/tool.rs", 1),
    // Reference-transaction fixtures invoke Git only inside disposable
    // repositories to drive real prepared/committed/aborted hook boundaries.
    ("spice/tests/reference_transaction_cli.rs", 1),
    ("spice/tests/reference_transaction_cli/movement.rs", 3),
];

/// Every git subcommand that can reach the network.
const NETWORK_SUBCOMMANDS: [&str; 5] = ["fetch", "push", "pull", "clone", "ls-remote"];

/// Network subcommands no production source may name at all.
///
/// The two the control plane does use are bounded by call site below. These
/// two have no sanctioned use anywhere outside a fixture: `pull` is the one
/// GIT-01 names outright, and `clone` would be a new way for a lane to reach a
/// remote without going through a boundary. The release authority may name
/// `ls-remote`, but only as inert argv returned to its parent control plane.
const UNSANCTIONED: [&str; 2] = ["pull", "clone"];

/// Source families that participate in selecting, acquiring, or publishing a proof.
const PROOF_GIT_PREFIXES: [&str; 5] = [
    "gate/src/core/",
    "gate/src/tree/",
    "gate/src/battery/repository_executor.rs",
    "spice/src/gate_command.rs",
    "spice/src/gate_command/",
];

/// Git operations that transfer caller-state ownership instead of observing it.
const PROOF_MUTATING_SUBCOMMANDS: [&str; 13] = [
    "fetch",
    "pull",
    "push",
    "rebase",
    "stash",
    "reset",
    "read-tree",
    "update-index",
    "commit-tree",
    "update-ref",
    "checkout",
    "merge",
    "cherry-pick",
];

/// The only production sources allowed to name a network subcommand.
const CONTROL_PLANE_SOURCES: [&str; 3] = [
    "dist/src/authority.rs",
    "spice/src/task_git.rs",
    "spice/src/task_git/command.rs",
];

/// Every occurrence of a network subcommand literal in the tracked Rust set.
///
/// Counted per occurrence rather than per line, because
/// `spice/src/task_git/command.rs` names both of the sanctioned subcommands on
/// one line and a line-counting scan would see one site where there are two.
const NETWORK_CALL_SITES: [(&str, &str, usize); 10] = [
    // The external release authority constructs exact inert Git argv but owns
    // no process runner. Its parent executes these operations at an existing
    // control-plane boundary after authored release approval. Each count
    // covers the single production constructor plus its co-located exact-argv
    // assertions.
    ("dist/src/authority.rs", "ls-remote", 4),
    ("dist/src/authority.rs", "push", 2),
    // The whole of GIT-01's production surface: one fetch, one push, both in
    // the control-plane module and both reached only through a boundary.
    ("spice/src/task_git.rs", "fetch", 1),
    ("spice/src/task_git.rs", "push", 1),
    // Not invocations. This pair is a `matches!` arm that recognizes the two
    // subcommands by name to give them a longer timeout, and issues nothing.
    ("spice/src/task_git/command.rs", "fetch", 1),
    ("spice/src/task_git/command.rs", "push", 1),
    // The task-git module's repository-backed fixtures clone, fetch, and push
    // only against disposable local bare remotes under their owned roots.
    ("spice/src/task_git/tests.rs", "clone", 1),
    ("spice/src/task_git/tests.rs", "fetch", 2),
    ("spice/src/task_git/tests.rs", "push", 5),
    // This fixture fetches only from its owned local bare remote after moving
    // the upstream branch, exercising divergent task-completion admission.
    (
        "spice/tests/reference_transaction_cli/movement.rs",
        "fetch",
        1,
    ),
];

/// The activation surface, which GIT-06 keeps free of git entirely.
const ACTIVATION_CRATE: &str = "steer/";

/// Tokens that would put git into the activation surface.
///
/// `steer/src/run.rs` does launch a process, but it launches the intercepted
/// command the shell was already running, chosen by the caller rather than by
/// activation. What GIT-06 forbids is activation choosing git for itself, so
/// the scan looks for git named in that crate at all.
///
/// The last token ends at the closing quote rather than spelling a whole word,
/// so that it catches the absolute form used everywhere else in this tree,
/// `"/usr/bin/git"`, which a quoted-word needle reads straight past. It matches
/// nothing in the activation crate today: the only occurrences of those three
/// letters there are inside `digit`, and none of them precede a quote.
const GIT_TOKENS: [&str; 3] = ["task_git", "git_shadow", "git\""];

/// What each public git entry point does to the repository.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Boundary {
    /// GIT-02, at claim: fast-forward to the baseline and record that commit.
    Claim,
    /// GIT-03, at phase completion: integrate and publish the task boundary.
    PhaseCompletion,
    /// GIT-04, at agent launch: advance opportunistically and never raise, and
    /// publish the per-worktree git shadow. Two writes, one boundary: the
    /// shadow composes the environment the lane runs under, and the advance
    /// fast-forwards the lane to the baseline when every rule allows it.
    Launch,
    /// Not a boundary. Reads the lane without moving it or reaching a remote.
    ReadOnly,
}

/// Every module git passes through, and every public entry point it offers.
///
/// This is also the module list the scan walks, so there is no second place
/// naming these files that could come to disagree with this one.
///
/// `capture` sits under `Claim` rather than beside the read-only pair despite
/// its own summary line saying it does not move the lane: it calls
/// `recover_pending`, which can run `read-tree --reset -u`, and it fetches. It
/// is reached from the claim path, which is where its boundary is recorded.
const ENTRY_POINTS: [(&str, &[(&str, Boundary)]); 2] = [
    (
        "spice/src/git_shadow.rs",
        &[("environment", Boundary::Launch)],
    ),
    (
        "spice/src/task_git.rs",
        &[
            ("advance_at_launch", Boundary::Launch),
            ("capture", Boundary::Claim),
            ("prepare_claim", Boundary::Claim),
            ("prepare_free", Boundary::ReadOnly),
            ("publish_claimed", Boundary::PhaseCompletion),
            ("stranded_commits", Boundary::ReadOnly),
        ],
    ),
];

/// The one production seam every occupant launch passes through.
///
/// The `Launch` disposition is a claim about what happens at runtime, and prose
/// cannot hold it: without reading this file, either Launch entry point could
/// be dropped from the resident and the assertions below would stay green while
/// the disposition went on describing behaviour the tree no longer has.
///
/// The needle is the fully qualified call, derived from the module path rather
/// than spelled out, so it cannot be satisfied by a same-named neighbour --
/// `launch_environment` lives in this very file. Importing a git module and
/// calling it unqualified therefore reds this test, which is the intended
/// trade: the seam is small and the qualified form is what makes it greppable.
const LAUNCH_SURFACE: &str = "spice/src/resident/lane_surface/launch.rs";

/// GIT-01: no source names a network subcommand outside the allowlist.
#[test]
fn every_tracked_network_subcommand_literal_is_allowlisted() {
    let sources = workspace_rust_sources();
    assert!(
        sources.contains_key(SELF_PATH),
        "{SELF_PATH} is not in the tracked set, so the carve-out below names \
         nothing and this test is scanning a tree that does not include itself"
    );

    let found = scan_network_sites(&sources);
    let declared = NETWORK_CALL_SITES
        .into_iter()
        .map(|(path, subcommand, count)| (path.to_owned(), subcommand.to_owned(), count))
        .collect::<BTreeSet<_>>();

    let added = found.difference(&declared).collect::<Vec<_>>();
    assert!(
        added.is_empty(),
        "git network call sites that no allowlist entry admits: {added:#?}\n\
         add each to NETWORK_CALL_SITES with a reason, or route it through a boundary"
    );
    let removed = declared.difference(&found).collect::<Vec<_>>();
    assert!(
        removed.is_empty(),
        "NETWORK_CALL_SITES claims call sites the tree no longer has: {removed:#?}\n\
         drop each entry so the allowlist keeps describing this tree"
    );
}

/// GIT-01: production reaches the network only from the control-plane module.
///
/// Read off the scan rather than off the allowlist, so it cannot be satisfied
/// by relabelling an entry. Whether a source is a fixture is likewise derived
/// from its path, not declared, for the same reason.
#[test]
fn production_names_a_network_subcommand_only_inside_the_control_plane() {
    let found = scan_network_sites(&workspace_rust_sources());

    let production = found
        .iter()
        .filter(|(path, _, _)| !is_fixture(path))
        .collect::<Vec<_>>();
    let stray = production
        .iter()
        .filter(|(path, _, _)| !CONTROL_PLANE_SOURCES.contains(&path.as_str()))
        .collect::<Vec<_>>();
    assert!(
        stray.is_empty(),
        "production sources outside the control plane name a git network \
         subcommand: {stray:#?}"
    );
    let unsanctioned = production
        .iter()
        .filter(|(_, subcommand, _)| UNSANCTIONED.contains(&subcommand.as_str()))
        .collect::<Vec<_>>();
    assert!(
        unsanctioned.is_empty(),
        "production names a network subcommand GIT-01 gives no boundary for: \
         {unsanctioned:#?}"
    );
}

/// Proof may inspect caller Git state, but may never take ownership of it.
#[test]
fn proof_sources_name_no_caller_mutating_git_operation() {
    let sources = workspace_rust_sources();
    let violations = sources
        .iter()
        .filter(|(path, _)| {
            *path == "gate/src/tree.rs"
                || PROOF_GIT_PREFIXES
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
        })
        .filter(|(path, _)| !is_fixture(path))
        .flat_map(|(path, source)| {
            // These two production modules keep their inline tests in one
            // terminal module. Test labels may describe a merge without
            // granting production permission to invoke one.
            let source = if matches!(
                path.as_str(),
                "gate/src/core/acquire.rs" | "gate/src/tree.rs"
            ) {
                source
                    .split_once("#[cfg(test)]\nmod tests")
                    .map_or(source.as_str(), |(production, _)| production)
            } else {
                source.as_str()
            };
            PROOF_MUTATING_SUBCOMMANDS
                .iter()
                .filter(move |subcommand| source.contains(&format!("\"{subcommand}\"")))
                .map(move |subcommand| format!("{path} names {subcommand}"))
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "proof sources name caller-mutating Git operations: {violations:#?}"
    );
    assert!(
        !sources
            .get("gate/src/core/acquire.rs")
            .expect("core acquisition source is tracked")
            .split_once("#[cfg(test)]\nmod tests")
            .map_or_else(
                || false,
                |(production, _)| production.contains("&[\"write-tree\"]")
            ),
        "proof must derive the staged tree identity without writing it to the caller object database"
    );
}

/// GIT-01 and GIT-06: three writing boundaries, and activation is not one.
#[test]
fn the_git_writing_boundaries_are_claim_completion_and_launch() {
    let sources = workspace_rust_sources();

    let found = public_entry_points(&sources);
    let declared = ENTRY_POINTS
        .into_iter()
        .flat_map(|(module, entries)| {
            entries
                .iter()
                .map(move |(symbol, _)| (module.to_owned(), (*symbol).to_owned()))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        found, declared,
        "the public git entry points changed; give every one of them a \
         boundary in ENTRY_POINTS, or none of the assertions below are about \
         the whole surface"
    );

    let writing = ENTRY_POINTS
        .into_iter()
        .flat_map(|(_, entries)| entries.iter().map(|(_, boundary)| *boundary))
        .filter(|boundary| *boundary != Boundary::ReadOnly)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        writing,
        BTreeSet::from([Boundary::Claim, Boundary::PhaseCompletion, Boundary::Launch]),
        "git is touched at exactly three control-plane boundaries"
    );

    let surface = sources
        .get(LAUNCH_SURFACE)
        .unwrap_or_else(|| panic!("{LAUNCH_SURFACE} is tracked"));
    for (module, entries) in ENTRY_POINTS {
        for (symbol, boundary) in entries {
            if *boundary != Boundary::Launch {
                continue;
            }
            let call = qualified_call(module, symbol);
            assert!(
                surface.contains(&call),
                "{symbol} is dispositioned Launch, but {LAUNCH_SURFACE} never \
                 calls {call}), so nothing reaches it when a lane starts"
            );
        }
    }

    let advance_call = qualified_call("spice/src/task_git.rs", "advance_at_launch");
    let note_assignment = format!("let git = {advance_call}&repository).to_string();");
    assert!(
        surface.contains(&note_assignment),
        "{LAUNCH_SURFACE} must retain the exact GIT-04 result instead of silently discarding it"
    );
    let reporter = surface
        .split_once("Arc::new(DeclarationReporter {")
        .and_then(|(_, tail)| tail.split_once("});"))
        .map(|(reporter, _)| reporter)
        .expect("the live launch seam constructs its declaration reporter");
    assert!(
        reporter.contains("git: git.clone(),"),
        "the retained GIT-04 note does not reach the durable declaration reporter"
    );
    let launch = surface
        .split_once("Ok(OccupantLaunch {")
        .and_then(|(_, tail)| tail.split_once("})"))
        .map(|(launch, _)| launch)
        .expect("the live launch seam returns its occupant launch");
    assert!(
        launch.lines().any(|line| line.trim() == "git,"),
        "the retained GIT-04 note does not reach the immediate launch result"
    );

    let activation = sources
        .iter()
        .filter(|(path, _)| path.starts_with(ACTIVATION_CRATE))
        .flat_map(|(path, text)| {
            GIT_TOKENS
                .into_iter()
                .filter(|token| text.contains(*token))
                .map(move |token| format!("{path} names {token}"))
        })
        .collect::<Vec<_>>();
    assert!(
        activation.is_empty(),
        "activation is not a fourth boundary, but git reaches it: {activation:#?}"
    );
}

/// This battery remains a named workspace target for scoped execution.
///
/// The three assertions close the question together with the fact that this
/// body is executing at all: the file is present in the selected workspace
/// source set, `xtask` is a workspace member so cargo builds its integration
/// tests,
/// `cargo test --locked -p xtask --test git_boundaries` therefore reaches it
/// without depending on a repository-wide verifier.
#[test]
fn this_battery_remains_a_scoped_workspace_target() {
    let root = workspace_root();
    let sources = workspace_rust_sources();
    assert!(
        sources.contains_key(SELF_PATH),
        "{SELF_PATH} is absent from the selected workspace source set"
    );

    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read workspace manifest");
    assert!(
        manifest.contains("\n    \"xtask\",\n"),
        "xtask is not a workspace member, so --workspace never builds this file"
    );
}

/// Repository test Git processes remain closed over an exact tracked inventory.
#[test]
fn repository_test_sources_do_not_spawn_or_wrap_git() {
    let sources = workspace_test_sources();
    let found_absolute = sources
        .iter()
        .filter(|(path, _)| path.as_str() != SELF_PATH)
        .filter_map(|(path, text)| {
            let count = text.matches("/usr/bin/git").count();
            (count > 0).then(|| (path.clone(), count))
        })
        .collect::<BTreeSet<_>>();
    let declared_absolute = INERT_ABSOLUTE_GIT_TEST_SITES
        .into_iter()
        .map(|(path, count)| (path.to_owned(), count))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        found_absolute, declared_absolute,
        "absolute Git spellings in tests changed; tests must inject parsed or repository state, and inert assertions must be declared exactly"
    );

    let found_processes = git_process_violations(&sources)
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert!(
        found_processes.is_empty(),
        "tests must use in-process repository state instead of Git process constructors: {found_processes:?}"
    );
}

#[test]
fn inline_rust_tests_are_selected_without_absorbing_production_items() {
    let source = r#"
fn production_before() {
    Command::new("git");
}

#[cfg(test)]
mod tests {
    fn process_fixture() {
        Command::new("git");
    }
}

fn production_after() {
    Command::new("git");
}
"#;
    let selected = selected_test_source("fixture/src/lib.rs", source)
        .expect("the inline test module is selected");
    assert!(selected.contains("process_fixture"));
    assert!(!selected.contains("production_before"));
    assert!(!selected.contains("production_after"));

    let inline = BTreeMap::from([("fixture/src/lib.rs".to_owned(), selected)]);
    assert_eq!(git_process_violations(&inline).len(), 1);

    let path_test = "fn fixture() { Command::new(\"git\"); }";
    assert_eq!(
        selected_test_source("fixture/tests/process.rs", path_test).as_deref(),
        Some(path_test)
    );
    let javascript = "execute(\"git\", [\"status\"]);";
    assert_eq!(
        selected_test_source("fixture/process.test.mjs", javascript).as_deref(),
        Some(javascript)
    );
}

fn git_process_violations(sources: &BTreeMap<String, String>) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, text) in sources
        .iter()
        .filter(|(path, _)| path.as_str() != SELF_PATH)
    {
        let compact = text
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        for needle in [
            "command::new(\"git\")",
            "command::new('git')",
            "command::new(git)",
            "execfile(\"git\"",
            "execfilesync(\"git\"",
            "execute(\"git\"",
            "spawn(\"git\"",
            "spawnsync(\"git\"",
        ] {
            if compact.contains(needle) {
                violations.push(format!("{path}: executable constructor contains {needle}"));
            }
        }
        for (index, line) in text.lines().enumerate() {
            let lower = line.trim().to_ascii_lowercase();
            if lower.starts_with("git ") || lower.contains("exec git ") || lower.contains("\\ngit ")
            {
                violations.push(format!(
                    "{path}:{}: shell wrapper contains {}",
                    index + 1,
                    line.trim()
                ));
            }
        }
    }
    violations
}

/// Whether a tracked source is test code rather than production.
///
/// Derived from the path so that no entry can be excused by how it is
/// labelled. A `tests.rs` module inside a source tree is test code even though
/// a `tests/` directory sweep alone would miss it.
///
/// The suffixes are anchored at a separator rather than left bare, so that a
/// production module whose name merely ends in those letters cannot excuse
/// itself into the fixture half. Both forms select the same files today.
fn is_fixture(path: &str) -> bool {
    path.ends_with("/tests.rs") || path.ends_with("_tests.rs") || path.contains("/tests/")
}

/// How an entry point is spelled when another crate module calls it.
///
/// Derived from the module's own path so the needle cannot drift away from the
/// file it is about: renaming or moving a git module changes both halves of
/// `ENTRY_POINTS` and this needle together.
fn qualified_call(module: &str, symbol: &str) -> String {
    let path = module
        .strip_prefix("spice/src/")
        .and_then(|rest| rest.strip_suffix(".rs"))
        .unwrap_or_else(|| panic!("{module} is not a module of the spice crate"));
    format!("spice::{}::{symbol}(", path.replace('/', "::"))
}

/// Every network subcommand literal in the tracked set, counted per file.
fn scan_network_sites(sources: &BTreeMap<String, String>) -> BTreeSet<(String, String, usize)> {
    let mut found = BTreeSet::new();
    for (path, text) in sources {
        if path == SELF_PATH {
            continue;
        }
        for subcommand in NETWORK_SUBCOMMANDS {
            let count = text.matches(&format!("\"{subcommand}\"")).count();
            if count > 0 {
                found.insert((path.clone(), subcommand.to_owned(), count));
            }
        }
    }
    found
}

/// The `pub fn` declarations of the git modules named by `ENTRY_POINTS`.
fn public_entry_points(sources: &BTreeMap<String, String>) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for (module, _) in ENTRY_POINTS {
        let text = sources
            .get(module)
            .unwrap_or_else(|| panic!("{module} is tracked"));
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("pub fn ") else {
                continue;
            };
            let symbol = rest
                .split(['(', '<'])
                .next()
                .expect("a split always yields a first part");
            found.insert((module.to_owned(), symbol.to_owned()));
        }
    }
    found
}

/// Every Rust source in the selected workspace tree.
///
/// Walking the compiled workspace gives direct and scoped execution one path.
/// In a development worktree the walk also sees untracked Rust scratch files
/// and therefore fails closed: an extra network literal can red the invariant,
/// but it cannot hide from it.
fn workspace_rust_sources() -> BTreeMap<String, String> {
    let root = workspace_root();
    let mut sources = BTreeMap::new();
    collect_rust_sources(&root, &root, &mut sources);
    sources
}

/// Every test-only source body in the selected workspace tree.
fn workspace_test_sources() -> BTreeMap<String, String> {
    let root = workspace_root();
    let mut sources = BTreeMap::new();
    collect_test_sources(&root, &root, &mut sources);
    sources
}

fn collect_test_sources(root: &Path, directory: &Path, sources: &mut BTreeMap<String, String>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("read source entry in {}: {error}", directory.display()));
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .unwrap_or_else(|error| panic!("inspect source path {}: {error}", path.display()));
        if kind.is_dir() {
            if !excluded_source_directory(&entry.file_name()) {
                collect_test_sources(root, &path, sources);
            }
            continue;
        }
        if !kind.is_file()
            || !matches!(
                path.extension().and_then(OsStr::to_str),
                Some("bash" | "cjs" | "js" | "jsx" | "mjs" | "py" | "rs" | "sh" | "ts" | "tsx")
            )
        {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("walked source stays beneath workspace")
            .to_str()
            .expect("test source path is UTF-8")
            .to_owned();
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read test source {relative}: {error}"));
        let selected = selected_test_source(&relative, &text);
        if let Some(selected) = selected {
            sources.insert(relative, selected);
        }
    }
}

fn selected_test_source(path: &str, source: &str) -> Option<String> {
    let named_test = is_fixture(path)
        || path
            .rsplit_once('/')
            .map_or(path, |(_, name)| name)
            .contains(".test.")
        || path == "xtask/src/app/source/modules.rs";
    if Path::new(path).extension() == Some(OsStr::new("rs")) {
        let inline = gate::test_support::rust_test_item_sources(source)
            .unwrap_or_else(|error| panic!("parse Rust test items in {path}: {error}"));
        if named_test {
            return Some(source.to_owned());
        }
        if !inline.is_empty() {
            return Some(inline.join("\n"));
        }
    }
    named_test.then(|| source.to_owned())
}

fn collect_rust_sources(root: &Path, directory: &Path, sources: &mut BTreeMap<String, String>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", directory.display()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("read source entry in {}: {error}", directory.display()));
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let kind = entry
            .file_type()
            .unwrap_or_else(|error| panic!("inspect source path {}: {error}", path.display()));
        if kind.is_dir() {
            if !excluded_source_directory(&entry.file_name()) {
                collect_rust_sources(root, &path, sources);
            }
            continue;
        }
        if !kind.is_file() || path.extension() != Some(OsStr::new("rs")) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("walked source stays beneath workspace")
            .to_str()
            .expect("Rust source path is UTF-8")
            .to_owned();
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read Rust source {relative}: {error}"));
        sources.insert(relative, text);
    }
}

fn excluded_source_directory(name: &OsStr) -> bool {
    [
        ".agents",
        ".git",
        ".spice",
        ".spice-rs",
        "node_modules",
        "stage-app",
        "target",
    ]
    .into_iter()
    .any(|excluded| name == OsStr::new(excluded))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent")
        .to_path_buf()
}
