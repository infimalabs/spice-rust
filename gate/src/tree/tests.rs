use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sill::Acquisition;

use super::{Selector, StagedAuthorityDifference, TreeError};

struct Repository {
    root: PathBuf,
}

impl Repository {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-tree-unit-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create a unit repository");
        let repository = Self { root };
        repository.git(&["init", "-q", "-b", "main"]);
        repository.git(&["config", "user.name", "Gate Unit"]);
        repository.git(&["config", "user.email", "gate@example.invalid"]);
        repository
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("create a fixture directory");
        fs::write(path, bytes).expect("write a fixture file");
    }

    fn git(&self, arguments: &[&str]) -> String {
        self.git_at(&self.root, arguments)
    }

    fn git_at(&self, directory: &Path, arguments: &[&str]) -> String {
        match arguments {
            ["init", ..] => crate::test_repository::initialize(directory),
            ["config", ..] => {}
            ["add", "--all"] => crate::test_repository::stage_all(directory),
            ["add", paths @ ..] => crate::test_repository::stage(directory, paths),
            ["commit", .., "-m", message] => {
                crate::test_repository::commit(directory, message);
            }
            ["rev-parse", "HEAD"] => return crate::test_repository::head(directory),
            ["mv", source, destination] => {
                crate::test_repository::move_path(directory, source, destination);
            }
            ["update-ref", reference, id] => {
                crate::test_repository::set_ref(&self.root, reference, id);
            }
            ["worktree", "add", "-q", "-b", branch, linked, "HEAD"] => {
                crate::test_repository::create_linked_worktree(
                    &self.root,
                    Path::new(linked),
                    branch,
                );
            }
            ["worktree", "remove", "--force", linked] => {
                crate::test_repository::remove_linked_worktree(&self.root, Path::new(linked));
            }
            ["pack-refs", "--all", option] => {
                crate::test_repository::pack_refs(&self.root, *option == "--prune");
            }
            ["branch", branch] => crate::test_repository::create_branch(&self.root, branch),
            ["symbolic-ref", "HEAD", reference] => {
                crate::test_repository::set_symbolic_head(&self.root, reference);
            }
            ["update-index", "--split-index"] => {
                crate::test_repository::split_index(&self.root);
            }
            ["checkout", "-q", "-b", branch, start] => {
                crate::test_repository::checkout_from(&self.root, branch, true, Some(start));
            }
            ["checkout", "-q", branch] => {
                crate::test_repository::checkout(&self.root, branch, false);
            }
            unsupported => panic!("unsupported in-process fixture operation {unsupported:?}"),
        }
        String::new()
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn staged_selection_carries_renames_from_its_captured_index() {
    let repository = Repository::new("captured-renames");
    repository.write("old.txt", b"selected\n");
    repository.commit("old path");
    repository.git(&["mv", "old.txt", "new.txt"]);

    let selected = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the renamed index");

    assert_eq!(selected.renames().destination("old.txt"), "new.txt");
    assert!(selected.publishes_state());
}

#[test]
fn staged_authority_distinguishes_exact_rename_base_and_lineage() {
    let repository = Repository::new("staged-authority-lineage");
    repository.write("old.txt", b"selected\n");
    repository.commit("first rename base");
    let first = Selector::staged()
        .materialize(&repository.root)
        .expect("capture the first staged authority");
    repository.git(&["commit", "--allow-empty", "-q", "-m", "second rename base"]);
    let second = Selector::staged()
        .materialize(&repository.root)
        .expect("capture an unchanged tree from a different rename base");
    assert_eq!(
        first
            .staged_authority()
            .expect("first selection is staged")
            .differences(
                second
                    .staged_authority()
                    .expect("second selection is staged"),
            ),
        [StagedAuthorityDifference::RenameBase],
    );

    repository.git(&["mv", "old.txt", "new.txt"]);
    let renamed = Selector::staged()
        .materialize(&repository.root)
        .expect("capture staged rename lineage");
    let authority = renamed
        .staged_authority()
        .expect("renamed selection is staged");
    let mut without_lineage = authority.clone();
    without_lineage.renames = Default::default();
    assert_eq!(
        authority.differences(&without_lineage),
        [StagedAuthorityDifference::RenameLineage],
    );
}

#[test]
fn staged_rename_similarity_is_fixed_independently_of_repository_config() {
    let repository = Repository::new("captured-rename-similarity");
    repository.write(
        "old.txt",
        b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n",
    );
    repository.commit("old path");
    repository.git(&["config", "diff.renames", "false"]);
    repository.git(&["config", "diff.renameLimit", "1"]);
    repository.git(&["mv", "old.txt", "new.txt"]);
    repository.write(
        "new.txt",
        b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n",
    );
    repository.git(&["add", "new.txt"]);

    let selected = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the modified rename");

    assert_eq!(selected.renames().destination("old.txt"), "new.txt");
}

#[test]
fn revision_selection_is_read_only_for_current_gate_state() {
    let repository = Repository::new("revision-state-authority");
    repository.write("selected.txt", b"selected\n");
    let revision = repository.commit("selected revision");

    let selected = Selector::revision(revision)
        .materialize(&repository.root)
        .expect("materialize the revision");

    assert!(!selected.publishes_state());
}

#[test]
fn head_movement_refuses_rename_lineage_from_a_different_base() {
    let repository = Repository::new("moving-rename-base");
    repository.write("old.txt", b"first\n");
    let first = repository.commit("first base");
    repository.write("anchor.txt", b"second\n");
    repository.commit("second base");
    repository.git(&["mv", "old.txt", "new.txt"]);

    let refused = Selector::staged()
        .materialize_with(&repository.root, |_| {
            repository.git(&["update-ref", "refs/heads/main", &first]);
        })
        .expect_err("a moved rename base must refuse the staged source");

    assert!(
        matches!(
            refused,
            TreeError::RevisionChanged { ref revision } if revision == "HEAD"
        ),
        "the sealed staged source refused with {refused:?}"
    );
}

#[test]
fn byte_identical_index_replacement_is_still_a_changed_source() {
    let repository = Repository::new("index-replacement");
    repository.write("selected.txt", b"selected\n");
    repository.git(&["add", "selected.txt"]);
    let index = repository.root.join(".git/index");

    let refused = Selector::staged()
        .materialize_with(&repository.root, |_| {
            let replacement = repository.root.join(".git/index.replacement");
            fs::copy(&index, &replacement).expect("copy an identical index");
            fs::rename(&replacement, &index).expect("replace the captured index");
        })
        .expect_err("index replacement must refuse the selection");

    assert!(matches!(refused, TreeError::IndexChanged { .. }));
}

#[test]
fn retained_staged_seal_detects_replacement_after_materialization() {
    let repository = Repository::new("retained-index-replacement");
    repository.write("selected.txt", b"selected\n");
    repository.git(&["add", "selected.txt"]);
    let selected = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize one retained staged source");
    let index = repository.root.join(".git/index");
    let replacement = repository.root.join(".git/index.replacement");
    fs::copy(&index, &replacement).expect("copy an identical replacement");
    fs::rename(&replacement, &index).expect("replace the live index");

    let refused = selected
        .verify_source()
        .expect_err("retained source seal must observe replacement");

    assert!(matches!(refused, TreeError::IndexChanged { .. }));
}

#[test]
fn linked_peer_ref_traffic_preserves_hook_and_verifier_staged_source_validation() {
    let repository = Repository::new("linked-peer-ref-traffic");
    repository.write("selected.txt", b"first\n");
    repository.commit("selected base");
    let linked = repository.root.with_file_name(format!(
        "{}-peer",
        repository
            .root
            .file_name()
            .expect("repository has a file name")
            .to_string_lossy()
    ));
    let linked_text = linked.to_str().expect("fixture path is Unicode");
    repository.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "peer-traffic",
        linked_text,
        "HEAD",
    ]);
    repository.write("selected.txt", b"staged\n");
    repository.git(&["add", "selected.txt"]);

    let selected = Selector::staged()
        .materialize_with(&repository.root, |_| {
            repository.git_at(
                &linked,
                &["commit", "--allow-empty", "-q", "-m", "capture traffic"],
            );
            repository.git_at(&linked, &["pack-refs", "--all", "--no-prune"]);
        })
        .expect("pre-commit source validation ignores unrelated peer refs");
    assert_eq!(
        fs::read(selected.root().join("selected.txt")).expect("read staged source"),
        b"staged\n"
    );

    repository.git_at(
        &linked,
        &["commit", "--allow-empty", "-q", "-m", "verifier traffic"],
    );
    repository.git_at(&linked, &["pack-refs", "--all", "--no-prune"]);
    selected
        .verify_source()
        .expect("verifier source validation ignores unrelated peer refs");

    drop(selected);
    repository.git(&["worktree", "remove", "--force", linked_text]);
}

#[test]
fn an_index_swap_and_restore_before_decode_is_refused_without_a_partial_root() {
    let repository = Repository::new("index-swap-restore");
    repository.write("selected.txt", b"first\n");
    repository.git(&["add", "selected.txt"]);
    let index = repository.root.join(".git/index");
    let original = repository.root.join(".git/index.original");
    fs::copy(&index, &original).expect("save the original index");
    repository.write("selected.txt", b"second\n");
    repository.git(&["add", "selected.txt"]);
    let alternate = repository.root.join(".git/index.alternate");
    fs::copy(&index, &alternate).expect("save the alternate index");
    fs::copy(&original, &index).expect("restore the original index before capture");

    let mut workspace = None;
    let refused = Selector::staged()
        .materialize_with(&repository.root, |captured_workspace| {
            workspace = Some(captured_workspace.to_path_buf());
            let held = repository.root.join(".git/index.held");
            fs::rename(&index, &held).expect("hold the captured index");
            fs::copy(&alternate, &index).expect("install the alternate index");
            fs::remove_file(&index).expect("remove the alternate index");
            fs::rename(&held, &index).expect("restore the captured index");
        })
        .expect_err("a transient index replacement must be named");

    assert!(matches!(refused, TreeError::IndexChanged { .. }));
    assert!(
        !workspace.expect("capture the private workspace").exists(),
        "a refused selection removes its private workspace"
    );
}

#[test]
fn a_branch_move_after_capture_refuses_the_captured_commit() {
    let repository = Repository::new("moving-revision");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    repository.commit("second");

    let refused = Selector::revision("main")
        .materialize_with(&repository.root, |_| {
            repository.git(&["update-ref", "refs/heads/main", &first]);
        })
        .expect_err("a moving branch must refuse the selection");

    assert!(
        matches!(
            refused,
            TreeError::RevisionChanged { ref revision } if revision == "main"
        ),
        "{refused:?}"
    );
}

#[test]
fn a_selected_packed_ref_move_after_capture_is_refused() {
    let repository = Repository::new("moving-packed-revision");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    repository.commit("second");
    repository.git(&["pack-refs", "--all", "--prune"]);

    let refused = Selector::revision("main")
        .materialize_with(&repository.root, |_| {
            repository.git(&["update-ref", "refs/heads/main", &first]);
        })
        .expect_err("movement of the selected packed ref must refuse");

    assert!(
        matches!(
            refused,
            TreeError::RevisionChanged { ref revision } if revision == "main"
        ),
        "{refused:?}"
    );
}

#[test]
fn symbolic_head_movement_after_capture_is_refused() {
    let repository = Repository::new("moving-symbolic-head");
    repository.write("selected.txt", b"selected\n");
    repository.commit("selected");
    repository.git(&["branch", "other"]);

    let refused = Selector::staged()
        .materialize_with(&repository.root, |_| {
            repository.git(&["symbolic-ref", "HEAD", "refs/heads/other"]);
        })
        .expect_err("movement of symbolic HEAD must refuse");

    assert!(matches!(
        refused,
        TreeError::RevisionChanged { revision } if revision == "HEAD"
    ));
}

#[test]
fn shallow_boundary_movement_after_capture_is_refused() {
    let repository = Repository::new("moving-shallow-boundary");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    let second = repository.commit("second");
    fs::write(repository.root.join(".git/shallow"), format!("{first}\n"))
        .expect("publish a shallow boundary");

    let refused = Selector::revision("HEAD")
        .materialize_with(&repository.root, |_| {
            fs::write(repository.root.join(".git/shallow"), format!("{second}\n"))
                .expect("move the shallow boundary");
        })
        .expect_err("movement of the shallow boundary must refuse");

    assert!(matches!(
        refused,
        TreeError::RevisionChanged { revision } if revision == "HEAD"
    ));
}

#[test]
fn a_ref_swap_and_restore_before_resolution_is_refused() {
    let repository = Repository::new("ref-swap-restore");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    let second = repository.commit("second");

    let refused = Selector::revision("main")
        .materialize_with(&repository.root, |_| {
            repository.git(&["update-ref", "refs/heads/main", &first]);
            repository.git(&["update-ref", "refs/heads/main", &second]);
        })
        .expect_err("a transient ref replacement must be named");

    assert!(
        matches!(
            refused,
            TreeError::RevisionChanged { ref revision } if revision == "main"
        ),
        "{refused:?}"
    );
}

#[test]
fn an_object_id_selection_ignores_unrelated_ref_movement() {
    let repository = Repository::new("object-id-stability");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    let second = repository.commit("second");

    let selected = Selector::revision(&second)
        .materialize_with(&repository.root, |_| {
            repository.git(&["update-ref", "refs/heads/main", &first]);
        })
        .expect("an object identity is independent of branch movement");

    assert_eq!(
        fs::read(selected.root().join("selected.txt")).expect("read selected object bytes"),
        b"second\n"
    );
}

#[test]
fn a_split_index_is_decoded_from_its_captured_shared_bytes() {
    let repository = Repository::new("split-index");
    repository.write("selected.txt", b"split\n");
    repository.git(&["add", "selected.txt"]);
    repository.git(&["update-index", "--split-index"]);

    let selected = Selector::staged()
        .materialize(&repository.root)
        .expect("decode the captured split index");

    assert_eq!(
        fs::read(selected.root().join("selected.txt")).expect("read split-index bytes"),
        b"split\n"
    );
}

#[test]
fn a_linked_worktree_revision_resolves_from_captured_worktree_metadata() {
    let repository = Repository::new("linked-revision");
    repository.write("selected.txt", b"linked\n");
    repository.commit("linked source");
    let linked = repository.root.with_file_name(format!(
        "{}-checkout",
        repository
            .root
            .file_name()
            .expect("repository has a file name")
            .to_string_lossy()
    ));
    let linked_text = linked.to_str().expect("fixture path is Unicode");
    repository.git(&["worktree", "add", "-q", "-b", "linked", linked_text, "HEAD"]);

    let selected = Selector::revision("HEAD")
        .materialize(&linked)
        .expect("resolve the linked worktree HEAD from captured metadata");

    assert_eq!(
        fs::read(selected.root().join("selected.txt")).expect("read linked revision bytes"),
        b"linked\n"
    );
    repository.git(&["worktree", "remove", "--force", linked_text]);
}

#[test]
fn packed_refs_reflogs_pseudorefs_and_ancestry_resolve_from_the_capture() {
    let repository = Repository::new("revision-source-forms");
    repository.write("selected.txt", b"first\n");
    let first = repository.commit("first");
    repository.write("selected.txt", b"second\n");
    repository.commit("second");

    let ancestor = Selector::revision("HEAD~1")
        .materialize(&repository.root)
        .expect("resolve ancestry from captured objects");
    let reflog = Selector::revision("HEAD@{1}")
        .materialize(&repository.root)
        .expect("resolve the captured reflog");
    repository.git(&["checkout", "-q", "-b", "global-search", &first]);
    repository.write("selected.txt", b"global\n");
    repository.commit("captured global needle");
    repository.git(&["checkout", "-q", "main"]);
    let global = Selector::revision(":/captured global needle")
        .materialize(&repository.root)
        .expect("resolve a global revision search from captured references");
    fs::write(
        repository.root.join(".git/MERGE_HEAD"),
        format!("{first}\n"),
    )
    .expect("write a pseudoref");
    let pseudoref = Selector::revision("MERGE_HEAD")
        .materialize(&repository.root)
        .expect("resolve the captured pseudoref");
    repository.git(&["pack-refs", "--all", "--prune"]);
    let packed = Selector::revision("main")
        .materialize(&repository.root)
        .expect("resolve the captured packed ref");

    for selected in [&ancestor, &reflog, &pseudoref] {
        assert_eq!(
            fs::read(selected.root().join("selected.txt")).expect("read the historical revision"),
            b"first\n"
        );
    }
    assert_eq!(
        fs::read(packed.root().join("selected.txt")).expect("read the packed revision"),
        b"second\n"
    );
    assert_eq!(
        fs::read(global.root().join("selected.txt")).expect("read the global revision"),
        b"global\n"
    );
}

/// A private stand-in for the temporary directory the reclaim sweeps.
///
/// The real one is shared with every other checkout on this machine, and the
/// reclaim removes what it finds there, so a fixture that pointed the sweep at
/// it would be deciding the fate of a peer's live gate run rather than its own
/// planted evidence.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-tree-reclaim-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create a reclaim scratch directory");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        super::make_removable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Plant a workspace shaped like the one a killed gate run leaves behind.
///
/// The planted tree is sealed through the production sealer rather than left
/// writable. A materialization seals what it measures, and a sealed directory
/// cannot be read in order to empty it, so a reclaim that never lifted the seal
/// would still sweep a writable imitation clean and prove nothing.
fn plant_workspace(scratch: &Path) -> PathBuf {
    let workspace = scratch.join("spice-gate-tree-4242-0");
    let root = workspace.join("root");
    fs::create_dir_all(&root).expect("plant a gate tree workspace");
    fs::write(root.join("selected.txt"), b"measured\n").expect("plant a selected file");
    super::make_immutable(&root).expect("seal the planted tree");
    workspace
}

/// The lock a live gate run would hold over this workspace.
///
/// Resolved through the production helper rather than spelled here, so a
/// fixture cannot prove that some lock contends while the reclaim consults a
/// different one.
fn planted_lock(workspace: &Path) -> sill::LockFile {
    super::workspace_lock(workspace).expect("the planted workspace names a lock")
}

#[test]
fn a_gate_tree_workspace_no_live_run_holds_is_reclaimed_rather_than_accumulating() {
    let scratch = Scratch::new("abandoned");
    let workspace = plant_workspace(&scratch.root);
    let lock = planted_lock(&workspace);
    // What a killed run leaves: the workspace, and its lock held by nobody.
    let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() else {
        panic!("a freshly planted workspace must be lockable");
    };
    drop(possession);
    let measured = workspace.join("root/selected.txt");

    super::reclaim_abandoned(&scratch.root);

    assert!(
        !measured.exists(),
        "the abandoned tree's bytes outlived the run that was killed holding them"
    );
    assert!(
        !workspace.exists(),
        "the abandoned workspace accumulated instead of being reclaimed"
    );
    assert!(
        !lock.path().exists(),
        "the lock outlived the workspace it names, accumulating one file per run"
    );
}

/// A workspace a live run is measuring out of is never removed.
///
/// The holder is a second acquisition of the very lock the reclaim consults.
/// `flock` grants per open file description, so this contends exactly as a
/// separate process would, which is what makes the decision drivable here
/// rather than something a fixture has to win a race to observe.
#[test]
fn a_gate_tree_workspace_a_live_run_holds_is_never_removed() {
    let scratch = Scratch::new("held");
    let workspace = plant_workspace(&scratch.root);
    let lock = planted_lock(&workspace);
    let Ok(Acquisition::Acquired(held)) = lock.try_acquire() else {
        panic!("a freshly planted workspace must be lockable");
    };
    let measured = workspace.join("root/selected.txt");

    super::reclaim_abandoned(&scratch.root);

    assert_eq!(
        fs::read(&measured).expect("the held tree's bytes survive the sweep"),
        b"measured\n",
        "a workspace a live run holds was removed out from under it"
    );
    assert!(
        lock.path().exists(),
        "the held workspace's lock was unlinked while its run still held it"
    );
    drop(held);
}

/// An unlocked workspace belongs to an older build, not to a dead run.
///
/// Every workspace this build mints is locked before it exists, so an unlocked
/// one was minted by a gate that predates the lock and is most likely a live
/// run of that gate sharing this temporary directory. Reclaiming it would
/// delete the tree that run is measuring, so it is left for an operator.
#[test]
fn a_gate_tree_workspace_with_no_lock_beside_it_is_left_alone() {
    let scratch = Scratch::new("unlocked");
    let workspace = plant_workspace(&scratch.root);
    let measured = workspace.join("root/selected.txt");

    super::reclaim_abandoned(&scratch.root);

    assert_eq!(
        fs::read(&measured).expect("an unlocked workspace survives the sweep"),
        b"measured\n",
        "a workspace an older build is measuring was removed out from under it"
    );
    assert!(
        !planted_lock(&workspace).path().exists(),
        "the sweep locked a workspace into ownership it was asked only to consider"
    );
}

/// A killed run can remove its workspace and die before unlinking its lock.
///
/// The fixture takes and releases the production lock without ever creating
/// the directory, leaving exactly that residue rather than spelling an
/// imitation lock file directly.
#[test]
fn a_parentless_gate_tree_lock_no_live_run_holds_is_reclaimed() {
    let scratch = Scratch::new("parentless-lock");
    let workspace = scratch.root.join("spice-gate-tree-4242-parentless");
    let lock = planted_lock(&workspace);
    let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() else {
        panic!("a fresh workspace name's lock must be acquirable");
    };
    drop(possession);
    assert!(!workspace.exists(), "the fixture must have no workspace");
    assert!(lock.path().is_file(), "the fixture must leave its lock");

    super::reclaim_abandoned(&scratch.root);

    assert!(
        !lock.path().exists(),
        "an unheld lock outlived the absent workspace it names"
    );
}

/// A live run locks a fresh name before it creates the workspace directory.
///
/// The sweep is driven while that production possession remains held, making
/// the pre-creation window deterministic rather than asking the test to race.
#[test]
fn a_parentless_gate_tree_lock_a_live_run_holds_is_never_unlinked() {
    let scratch = Scratch::new("held-parentless-lock");
    let workspace = scratch.root.join("spice-gate-tree-4242-precreate");
    let lock = planted_lock(&workspace);
    let Ok(Acquisition::Acquired(held)) = lock.try_acquire() else {
        panic!("a fresh workspace name's lock must be acquirable");
    };
    assert!(
        !workspace.exists(),
        "the live run has not created its workspace"
    );

    super::reclaim_abandoned(&scratch.root);

    assert!(
        lock.path().exists(),
        "the sweep unlinked a live run's lock before its workspace existed"
    );
    assert!(
        matches!(lock.try_acquire(), Ok(Acquisition::Contended)),
        "the live run's production possession must still guard the name"
    );
    drop(held);
}

/// Materializing a tree is what runs the reclaim.
///
/// The tests above drive [`super::reclaim_abandoned`] directly, which pins what
/// it decides and says nothing about whether production ever calls it. This one
/// plants into the real temporary directory -- the one a selection sweeps --
/// and lets an ordinary selection be the only thing that touches it.
///
/// The planted name carries a word no minted name contains, so it can never be
/// a workspace a concurrent selection in this process is using, and a peer
/// sweeping the same shared directory can only reach the same verdict.
#[test]
fn materializing_a_tree_reclaims_an_abandoned_workspace() {
    static NEXT_ABANDONED: AtomicU64 = AtomicU64::new(0);
    let repository = Repository::new("reclaim-on-materialize");
    repository.write("selected.txt", b"selected\n");
    repository.git(&["add", "selected.txt"]);
    let abandoned = std::env::temp_dir().join(format!(
        "spice-gate-tree-{}-abandoned-{}",
        std::process::id(),
        NEXT_ABANDONED.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&abandoned).expect("plant an abandoned workspace beside the real ones");
    let lock = planted_lock(&abandoned);
    let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() else {
        panic!("a freshly planted workspace must be lockable");
    };
    drop(possession);

    let selected = Selector::staged()
        .materialize(&repository.root)
        .expect("materialize the staged index");

    assert!(
        !abandoned.exists(),
        "selecting a tree left an abandoned workspace behind in the shared directory"
    );
    assert!(
        !lock.path().exists(),
        "selecting a tree left an abandoned workspace's lock behind"
    );
    assert!(
        selected.root().join("selected.txt").is_file(),
        "the selection that swept still materialized its own tree"
    );
}
