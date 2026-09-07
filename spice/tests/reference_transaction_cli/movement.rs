use super::*;

struct Movement {
    scratch: Scratch,
    head: String,
    candidate: String,
    tree: String,
}

impl Movement {
    fn new(label: &str) -> Self {
        let scratch = Scratch::repository(label);
        configure_identity(scratch.path());
        git(scratch.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
        fs::write(scratch.join("tracked"), "base\n").unwrap();
        git(scratch.path(), &["add", "tracked"]);
        git(scratch.path(), &["commit", "-m", "base"]);
        let head = git(scratch.path(), &["rev-parse", "HEAD"]);
        fs::write(scratch.join("tracked"), "candidate\n").unwrap();
        git(scratch.path(), &["add", "tracked"]);
        let tree = git(scratch.path(), &["write-tree"]);
        let candidate = git(
            scratch.path(),
            &["commit-tree", &tree, "-p", &head, "-m", "candidate"],
        );
        git(scratch.path(), &["reset", "--hard", "HEAD"]);
        Self {
            scratch,
            head,
            candidate,
            tree,
        }
    }

    fn install_hook(&self) {
        self.scratch.executable(
            "hooks/reference-transaction",
            format!(
                "#!/bin/sh\nexec '{}' gate reference-transaction \"$@\"\n",
                spice()
            )
            .as_bytes(),
        );
        git(
            self.scratch.path(),
            &[
                "config",
                "core.hooksPath",
                self.scratch.join("hooks").to_str().unwrap(),
            ],
        );
    }

    fn admit(&self, branch: &str) {
        publish_admission(self.scratch.path(), &self.tree, branch, Some(&self.head));
    }

    fn assert_candidate(&self) {
        assert_eq!(
            git(self.scratch.path(), &["rev-parse", "HEAD"]),
            self.candidate
        );
        assert_eq!(
            fs::read_to_string(self.scratch.join("tracked")).unwrap(),
            "candidate\n"
        );
        assert_eq!(
            git(self.scratch.path(), &["diff", "HEAD", "--", "tracked"]),
            ""
        );
    }
}

#[test]
fn reset_to_head_uses_the_held_ref_even_when_git_queues_zero_as_old() {
    let fixture = Movement::new("reset-held-ref");
    fixture.admit("refs/heads/main");
    git(
        fixture.scratch.path(),
        &["reset", "--hard", &fixture.candidate],
    );
    fixture.install_hook();
    fs::write(fixture.scratch.join("tracked"), "local edits\n").unwrap();
    git(fixture.scratch.path(), &["reset", "--hard", "HEAD"]);
    fixture.assert_candidate();
    let zero = "0".repeat(fixture.head.len());
    assert_success(
        &invoke_prepared(
            fixture.scratch.path(),
            &format!("{zero} {} refs/heads/main\n", fixture.candidate),
        ),
        "unchanged ref with unchecked old value",
    );
}

#[test]
fn fast_forward_and_upstream_reset_accept_another_lanes_admission() {
    for operation in ["merge", "reset"] {
        let fixture = Movement::new(operation);
        fixture.admit("refs/heads/producer");
        git(
            fixture.scratch.path(),
            &["update-ref", "refs/remotes/origin/main", &fixture.candidate],
        );
        git(
            fixture.scratch.path(),
            &["config", "branch.main.remote", "origin"],
        );
        git(
            fixture.scratch.path(),
            &["config", "branch.main.merge", "refs/heads/main"],
        );
        fixture.install_hook();
        let mode = if operation == "merge" {
            "--ff-only"
        } else {
            "--hard"
        };
        git(
            fixture.scratch.path(),
            &[operation, mode, "refs/remotes/origin/main"],
        );
        fixture.assert_candidate();
    }
}

#[test]
fn existing_history_in_branches_and_tags_does_not_need_new_admission() {
    for reference in ["refs/heads/producer", "refs/tags/published"] {
        let fixture = Movement::new("existing-history");
        fixture.admit("refs/heads/producer");
        let descendant = git(
            fixture.scratch.path(),
            &[
                "commit-tree",
                &fixture.tree,
                "-p",
                &fixture.candidate,
                "-m",
                "descendant",
            ],
        );
        git(
            fixture.scratch.path(),
            &["update-ref", reference, &descendant],
        );
        fixture.install_hook();
        git(
            fixture.scratch.path(),
            &["reset", "--hard", &fixture.candidate],
        );
        fixture.assert_candidate();
    }
}

#[test]
fn the_same_tree_and_parent_admission_is_independent_of_branch_name() {
    let fixture = Movement::new("new-commit-admission");
    fixture.admit("refs/heads/producer");
    let existing = git(
        fixture.scratch.path(),
        &[
            "commit-tree",
            &fixture.tree,
            "-p",
            &fixture.head,
            "-m",
            "different commit",
        ],
    );
    git(
        fixture.scratch.path(),
        &["update-ref", "refs/heads/producer", &existing],
    );
    fixture.install_hook();
    let output = Command::new("/usr/bin/git")
        .args([
            "update-ref",
            "refs/heads/main",
            &fixture.candidate,
            &fixture.head,
        ])
        .current_dir(fixture.scratch.path())
        .output()
        .unwrap();
    assert_success(&output, "branch-independent candidate admission");
    assert_eq!(
        git(fixture.scratch.path(), &["rev-parse", "HEAD"]),
        fixture.candidate
    );
    let credit = evidence_store(fixture.scratch.path())
        .admission_credit(&transaction_id(
            &fixture.head,
            &fixture.candidate,
            "refs/heads/main",
        ))
        .unwrap()
        .expect("new commit retains exact admission credit");
    assert_eq!(
        credit.commits().collect::<Vec<_>>(),
        [fixture.candidate.as_str()]
    );
}

#[test]
fn task_landing_merge_matches_admission_by_first_parent_not_branch_tip() {
    for branch in ["agent-one", "nested/agent-two"] {
        let fixture = Movement::new("task-landing-first-parent");
        fixture.admit("refs/heads/proof-producer");
        git(
            fixture.scratch.path(),
            &["update-ref", "refs/remotes/origin/main", &fixture.head],
        );
        let landing = git(
            fixture.scratch.path(),
            &[
                "commit-tree",
                &fixture.tree,
                "-p",
                &fixture.head,
                "-p",
                &fixture.candidate,
                "-m",
                "task landing",
            ],
        );
        let reference = format!("refs/heads/{branch}");
        git(
            fixture.scratch.path(),
            &["update-ref", &reference, &fixture.candidate],
        );
        git(
            fixture.scratch.path(),
            &["symbolic-ref", "HEAD", &reference],
        );
        git(
            fixture.scratch.path(),
            &["reset", "--hard", &fixture.candidate],
        );
        fixture.install_hook();

        git(
            fixture.scratch.path(),
            &["update-ref", &reference, &landing, &fixture.candidate],
        );
        assert_eq!(git(fixture.scratch.path(), &["rev-parse", "HEAD"]), landing);
        assert_eq!(
            fs::read_to_string(fixture.scratch.join("tracked")).unwrap(),
            "candidate\n"
        );
    }
}

#[test]
fn existing_history_still_cannot_abandon_published_commits() {
    let fixture = Movement::new("protected-existing-history");
    git(
        fixture.scratch.path(),
        &["reset", "--hard", &fixture.candidate],
    );
    git(
        fixture.scratch.path(),
        &["update-ref", "refs/remotes/origin/main", &fixture.candidate],
    );
    git(
        fixture.scratch.path(),
        &["config", "branch.main.remote", "origin"],
    );
    git(
        fixture.scratch.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    fixture.install_hook();
    let output = Command::new("/usr/bin/git")
        .args(["reset", "--soft", &fixture.head])
        .current_dir(fixture.scratch.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(128), "{}", streams(&output));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refused to abandon upstream-merged"));
    fixture.assert_candidate();
}
