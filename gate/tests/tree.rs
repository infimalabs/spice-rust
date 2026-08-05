//! Git selections exercised through real repositories and real parser processes.

use std::fs;
#[cfg(target_family = "unix")]
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use gate::{
    Bounds, Ignore, Jurisdiction, Metric, Parser, Policy, Ratchet, Run, RunError, Selector,
    TreeError,
};
use sill::Deadlines;

mod support;

use support::Tree;

const GIT: &str = "/usr/bin/git";
/// Keep fixture Git child lifetimes disjoint under translated amd64 execution.
static FIXTURE_GIT_BARRIER: Mutex<()> = Mutex::new(());
const MUTATING_COUNTER: &str = "/bin/chmod u+w \"$1\"; \
     printf 'changed by parser\\n' >> \"$1\"; printf '1\\n'";

fn git(tree: &Tree, arguments: &[&str]) -> String {
    let _fixture_git_guard = FIXTURE_GIT_BARRIER
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let output = Command::new(GIT)
        .args(arguments)
        .current_dir(&tree.root)
        .env_clear()
        .env("HOME", &tree.root)
        .output()
        .unwrap_or_else(|error| panic!("run git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "git {arguments:?} exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git fixture output is UTF-8")
        .trim()
        .to_owned()
}

fn repository(label: &str) -> Tree {
    let tree = Tree::new(label);
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.name", "Gate Fixture"]);
    git(&tree, &["config", "user.email", "gate@example.invalid"]);
    tree
}

fn commit_all(tree: &Tree, message: &str) -> String {
    git(tree, &["add", "--all"]);
    git(tree, &["commit", "-q", "-m", message]);
    git(tree, &["rev-parse", "HEAD"])
}

fn bounds() -> Bounds {
    Bounds::new(
        "selected-tree-fixture",
        Deadlines::new(
            Duration::from_secs(30),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .expect("declare deadlines"),
        Duration::from_millis(100),
        64 * 1024,
    )
    .expect("declare bounds")
}

#[test]
fn staged_and_revision_sources_never_read_checkout_only_bytes() {
    let tree = repository("git-sources");
    tree.write("src/lib.rs", "committed\n");
    let committed = commit_all(&tree, "committed state");

    tree.write("src/lib.rs", "staged\n");
    tree.write("bin/tool.sh", "#!/bin/sh\nprintf 'staged tool\\n'\n");
    #[cfg(target_family = "unix")]
    fs::set_permissions(tree.path("bin/tool.sh"), fs::Permissions::from_mode(0o755))
        .expect("make the staged fixture executable");
    git(&tree, &["add", "src/lib.rs", "bin/tool.sh"]);
    tree.write("src/lib.rs", "unstaged\n");
    tree.write("notes.untracked", "untracked\n");

    let staged = Selector::staged()
        .materialize(&tree.root)
        .expect("materialize the captured index");
    let repeated = Selector::staged()
        .materialize(&tree.root)
        .expect("materialize the same index again");
    let revision = Selector::revision(&committed)
        .materialize(&tree.root)
        .expect("materialize the committed tree");
    let repeated_revision = Selector::revision(&committed)
        .materialize(&tree.root)
        .expect("materialize the same commit again");

    assert_eq!(
        fs::read(staged.root().join("src/lib.rs")).expect("read selected staged bytes"),
        b"staged\n"
    );
    assert_eq!(
        fs::read(revision.root().join("src/lib.rs")).expect("read selected revision bytes"),
        b"committed\n"
    );
    assert!(!staged.root().join("notes.untracked").exists());
    assert!(!revision.root().join("bin/tool.sh").exists());

    let ignore = Ignore::parse("").expect("declare an empty ignore policy");
    let staged_walk = Jurisdiction::walk(staged.root(), &ignore).expect("walk staged selection");
    let repeated_walk =
        Jurisdiction::walk(repeated.root(), &ignore).expect("walk repeated selection");
    let revision_walk =
        Jurisdiction::walk(revision.root(), &ignore).expect("walk revision selection");
    let repeated_revision_walk = Jurisdiction::walk(repeated_revision.root(), &ignore)
        .expect("walk repeated revision selection");
    assert_eq!(staged_walk.inventory(), repeated_walk.inventory());
    assert_eq!(
        staged_walk.tree_digest().expect("digest staged selection"),
        repeated_walk
            .tree_digest()
            .expect("digest repeated selection")
    );
    assert_eq!(
        revision_walk.inventory(),
        repeated_revision_walk.inventory()
    );
    assert_eq!(
        revision_walk
            .tree_digest()
            .expect("digest revision selection"),
        repeated_revision_walk
            .tree_digest()
            .expect("digest repeated revision selection")
    );

    #[cfg(target_family = "unix")]
    {
        assert_eq!(
            fs::metadata(staged.root())
                .expect("inspect selected root")
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            fs::metadata(staged.root().join("src/lib.rs"))
                .expect("inspect selected regular file")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            fs::metadata(staged.root().join("bin/tool.sh"))
                .expect("inspect selected executable")
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
    }

    let staged_root = staged.root().to_path_buf();
    drop(staged);
    assert!(
        !staged_root.exists(),
        "dropping a selection removes its root"
    );
}

#[test]
fn revision_refusals_name_the_requested_revision() {
    let tree = repository("revision-refusals");
    tree.write("tracked.txt", "one\n");
    commit_all(&tree, "one commit");

    let absent = Selector::revision("does-not-exist")
        .materialize(&tree.root)
        .expect_err("an absent revision is refused");
    assert!(matches!(
        &absent,
        TreeError::RevisionUnresolved { revision, .. } if revision == "does-not-exist"
    ));
    assert!(absent.to_string().contains("does-not-exist"));

    let non_commit = Selector::revision("HEAD^{tree}")
        .materialize(&tree.root)
        .expect_err("a tree revision is refused");
    assert!(matches!(
        &non_commit,
        TreeError::RevisionNotCommit { revision, .. } if revision == "HEAD^{tree}"
    ));
    assert!(non_commit.to_string().contains("HEAD^{tree}"));

    let range = Selector::revision("HEAD..HEAD")
        .materialize(&tree.root)
        .expect_err("a revision range is refused");
    assert!(matches!(range, TreeError::RevisionUnresolved { .. }));

    git(&tree, &["branch", "colliding"]);
    git(&tree, &["tag", "colliding"]);
    let ambiguous = Selector::revision("colliding")
        .materialize(&tree.root)
        .expect_err("an ambiguous branch and tag name is refused");
    assert!(matches!(
        ambiguous,
        TreeError::RevisionUnresolved { revision, .. } if revision == "colliding"
    ));
}

#[cfg(target_family = "unix")]
#[test]
fn symlinks_and_submodules_are_named_before_materialization() {
    let tree = repository("unsupported-modes");
    tree.write("target.txt", "target\n");
    commit_all(&tree, "ordinary file");

    symlink("target.txt", tree.path("linked.txt")).expect("create a fixture symlink");
    git(&tree, &["add", "linked.txt"]);
    let linked = Selector::staged()
        .materialize(&tree.root)
        .expect_err("a staged symlink is refused");
    assert!(matches!(
        linked,
        TreeError::Symlink { path } if path == "linked.txt"
    ));

    git(&tree, &["reset", "-q", "--hard", "HEAD"]);
    let commit = git(&tree, &["rev-parse", "HEAD"]);
    let cacheinfo = format!("160000,{commit},vendor/dependency");
    git(&tree, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
    let submodule = Selector::staged()
        .materialize(&tree.root)
        .expect_err("a staged submodule is refused");
    assert!(matches!(
        submodule,
        TreeError::Submodule { path } if path == "vendor/dependency"
    ));
}

#[test]
fn a_missing_staged_blob_produces_no_selected_tree() {
    let tree = repository("missing-object");
    tree.write("lost.txt", "lost\n");
    git(&tree, &["add", "lost.txt"]);
    let listing = git(&tree, &["ls-files", "--stage", "lost.txt"]);
    let object = listing
        .split_whitespace()
        .nth(1)
        .expect("the staged listing carries an object id");
    let object_path = tree
        .root
        .join(".git/objects")
        .join(&object[..2])
        .join(&object[2..]);
    fs::remove_file(&object_path).expect("remove the loose staged object");

    let refused = Selector::staged()
        .materialize(&tree.root)
        .expect_err("a missing staged blob is refused");
    assert!(matches!(
        refused,
        TreeError::MissingObject { path, .. } if path == "lost.txt"
    ));
}

#[test]
fn run_snapshot_integrity_remains_the_second_door() {
    let tree = repository("snapshot-second-door");
    tree.write("src/lib.rs", "one\n");
    git(&tree, &["add", "src/lib.rs"]);
    let selected = Selector::staged()
        .materialize(&tree.root)
        .expect("materialize staged bytes");

    let metric = Metric::new(
        "rust.lines",
        "rust",
        "lines in selected Rust files",
        Parser::new("/bin/sh", ["-c", MUTATING_COUNTER, "sh"]).expect("declare a mutating parser"),
        "*.rs\n",
        "",
    )
    .expect("declare a metric");
    let policy = Policy::new(
        Ignore::parse("").expect("declare an empty ignore policy"),
        [metric],
    )
    .expect("declare a policy");
    let ratchet = Ratchet::parse("rust.lines 1\n").expect("declare a ratchet");

    let refused = Run::adjudicate(&policy, selected.root(), &ratchet, &bounds())
        .expect_err("the parser changed Run's own snapshot");
    assert!(matches!(
        refused,
        RunError::SnapshotChanged { metric, .. } if metric == "rust.lines"
    ));
    assert_eq!(
        fs::read(selected.root().join("src/lib.rs")).expect("read original selection"),
        b"one\n",
        "Run's disposable snapshot shields the selected source root"
    );
}
