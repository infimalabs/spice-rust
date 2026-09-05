use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::repository::Repository;

use super::{CandidateTree, capture_candidate_tree, parse_candidate_tree};

const GIT: &str = "/usr/bin/git";

#[test]
fn ordinary_staged_content_resolves_its_candidate_tree() {
    let fixture = Fixture::new("ordinary");
    fixture.write("tracked.txt", b"candidate\n");
    fixture.git(&["add", "tracked.txt"]);

    let candidate = fixture.capture(None);

    assert_eq!(candidate.as_str(), fixture.tree(None));
}

#[test]
fn alternate_index_is_the_only_candidate_tree_authority() {
    let fixture = Fixture::new("alternate-index");
    fixture.write("tracked.txt", b"opening\n");
    fixture.commit_all("opening");
    let alternate = fixture.scratch.join("alternate.index");
    fixture.git_with_index(&["read-tree", "HEAD"], Some(&alternate));
    fixture.write("tracked.txt", b"alternate candidate\n");
    fixture.git_with_index(&["add", "tracked.txt"], Some(&alternate));

    let candidate = fixture.capture(Some(&alternate));

    assert_eq!(candidate.as_str(), fixture.tree(Some(&alternate)));
    assert_ne!(candidate.as_str(), fixture.tree(None));
}

#[test]
fn staged_deletion_changes_the_candidate_tree() {
    let fixture = Fixture::new("deletion");
    fixture.write("deleted.txt", b"opening\n");
    fixture.write("retained.txt", b"retained\n");
    fixture.commit_all("opening");
    let opening = fixture.tree(None);
    fs::remove_file(fixture.main.join("deleted.txt")).expect("remove tracked fixture file");
    fixture.git(&["add", "--update"]);

    let candidate = fixture.capture(None);

    assert_eq!(candidate.as_str(), fixture.tree(None));
    assert_ne!(candidate.as_str(), opening);
}

#[test]
fn staged_mode_change_changes_the_candidate_tree() {
    let fixture = Fixture::new("mode-change");
    fixture.write("tool.sh", b"#!/bin/sh\nexit 0\n");
    fixture.commit_all("opening");
    let opening = fixture.tree(None);
    let path = fixture.main.join("tool.sh");
    let mut permissions = fs::metadata(&path)
        .expect("read fixture mode")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("change fixture mode");
    fixture.git(&["add", "tool.sh"]);

    let candidate = fixture.capture(None);

    assert_eq!(candidate.as_str(), fixture.tree(None));
    assert_ne!(candidate.as_str(), opening);
    assert!(
        fixture
            .git_text(&["ls-files", "--stage", "tool.sh"], None)
            .starts_with("100755 "),
        "candidate index did not retain the executable mode"
    );
}

#[test]
fn linked_worktree_uses_its_own_staged_index() {
    let fixture = Fixture::new("linked-worktree");
    fixture.write("tracked.txt", b"opening\n");
    fixture.commit_all("opening");
    fixture.git(&[
        "worktree",
        "add",
        "-b",
        "candidate-lane",
        fixture.linked.to_str().expect("linked path is UTF-8"),
    ]);
    fs::write(fixture.linked.join("tracked.txt"), b"linked candidate\n")
        .expect("write linked candidate");
    git(&fixture.linked, &["add", "tracked.txt"], None);
    let repository = Repository::discover(&fixture.linked).expect("discover linked worktree");

    let candidate =
        capture_candidate_tree(&repository, None).expect("capture linked candidate tree");

    assert_eq!(
        candidate.as_str(),
        git_text(&fixture.linked, &["write-tree"], None)
    );
    assert_ne!(candidate.as_str(), fixture.tree(None));
}

#[test]
fn candidate_tree_parser_rejects_noncanonical_output() {
    for output in [
        b"0123456789012345678901234567890123456789".as_slice(),
        b"0123456789012345678901234567890123456789\nextra\n".as_slice(),
        b"012345678901234567890123456789012345678G\n".as_slice(),
    ] {
        let error = parse_candidate_tree(output).expect_err("noncanonical identity is refused");
        assert_eq!(error.reason(), "gate::candidate-tree-refused");
    }
}

struct Fixture {
    scratch: PathBuf,
    main: PathBuf,
    linked: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let scratch = loop {
            let candidate = std::env::temp_dir().join(format!(
                "spice-candidate-tree-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create candidate-tree fixture: {error}"),
            }
        };
        let main = scratch.join("main");
        let linked = scratch.join("linked");
        fs::create_dir(&main).expect("create main worktree");
        git(&main, &["init", "-b", "main"], None);
        git(&main, &["config", "core.filemode", "true"], None);
        Self {
            scratch,
            main,
            linked,
        }
    }

    fn write(&self, path: &str, contents: &[u8]) {
        fs::write(self.main.join(path), contents).expect("write fixture path");
    }

    fn git(&self, arguments: &[&str]) {
        git(&self.main, arguments, None);
    }

    fn git_with_index(&self, arguments: &[&str], index: Option<&Path>) {
        git(&self.main, arguments, index);
    }

    fn git_text(&self, arguments: &[&str], index: Option<&Path>) -> String {
        git_text(&self.main, arguments, index)
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&[
            "-c",
            "user.name=Spice Test",
            "-c",
            "user.email=spice@example.invalid",
            "commit",
            "-m",
            message,
        ]);
    }

    fn capture(&self, index: Option<&Path>) -> CandidateTree {
        let repository = Repository::discover(&self.main).expect("discover fixture repository");
        capture_candidate_tree(&repository, index.map(Path::as_os_str))
            .expect("capture candidate tree")
    }

    fn tree(&self, index: Option<&Path>) -> String {
        self.git_text(&["write-tree"], index)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.scratch).expect("remove candidate-tree fixture");
    }
}

fn git(repository: &Path, arguments: &[&str], index: Option<&Path>) {
    let _ = git_output(repository, arguments, index);
}

fn git_text(repository: &Path, arguments: &[&str], index: Option<&Path>) -> String {
    String::from_utf8(git_output(repository, arguments, index))
        .expect("Git output is UTF-8")
        .trim_end()
        .to_owned()
}

fn git_output(repository: &Path, arguments: &[&str], index: Option<&Path>) -> Vec<u8> {
    let mut command = Command::new(GIT);
    command.arg("-C").arg(repository).args(arguments);
    match index {
        Some(index) => {
            command.env("GIT_INDEX_FILE", index);
        }
        None => {
            command.env_remove("GIT_INDEX_FILE");
        }
    }
    let output = command.output().expect("launch Git fixture command");
    assert!(
        output.status.success(),
        "Git {:?} failed: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
