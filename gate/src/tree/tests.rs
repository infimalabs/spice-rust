use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use super::{Selector, TreeError};

/// Keep fixture Git child lifetimes disjoint under translated amd64 execution.
static FIXTURE_GIT_BARRIER: Mutex<()> = Mutex::new(());

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
        let _fixture_git_guard = FIXTURE_GIT_BARRIER
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let output = Command::new("/usr/bin/git")
            .args(arguments)
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", &self.root)
            .output()
            .unwrap_or_else(|source| panic!("run Git {arguments:?}: {source}"));
        assert!(
            output.status.success(),
            "Git {arguments:?} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("Git output is UTF-8")
            .trim()
            .to_owned()
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

    assert!(matches!(
        refused,
        TreeError::RevisionChanged { revision } if revision == "main"
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

    assert!(matches!(
        refused,
        TreeError::RevisionChanged { revision } if revision == "main"
    ));
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
}
