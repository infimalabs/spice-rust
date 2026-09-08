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
fn task_done_lands_owned_divergent_commits_and_hook_refusal_is_atomic() {
    exercise_task_done("worktree-alpha", "agent-one", true);
    exercise_task_done("worktree-beta", "nested/agent-two", true);
    exercise_task_done("worktree-mismatch", "mismatched-agent", false);
    exercise_hook_guarded_mismatch();
}

fn exercise_task_done(worktree_label: &str, branch: &str, reference_admission_matches: bool) {
    let scratch = completion_repository(worktree_label);
    let root = scratch.path();
    let base = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-b", "published-upstream"]);
    fs::write(scratch.join("upstream"), "upstream\n").unwrap();
    git(root, &["add", "upstream"]);
    git(root, &["commit", "-m", "upstream"]);
    let upstream = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "published-upstream"]);
    git(root, &["checkout", "-b", branch, &base]);
    git(root, &["remote", "add", "origin", root.to_str().unwrap()]);
    git(
        root,
        &["config", &format!("branch.{branch}.remote"), "origin"],
    );
    git(
        root,
        &[
            "config",
            &format!("branch.{branch}.merge"),
            "refs/heads/main",
        ],
    );
    fs::write(scratch.join("agent"), "agent\n").unwrap();
    git(root, &["add", "agent"]);
    git(root, &["commit", "-m", "agent"]);
    let agent = git(root, &["rev-parse", "HEAD"]);
    let integrated_tree = git(
        root,
        &[
            "merge-tree",
            "--write-tree",
            "--no-messages",
            &agent,
            &upstream,
        ],
    );

    publish_admission(
        root,
        &integrated_tree,
        branch,
        Some(if reference_admission_matches {
            &upstream
        } else {
            &base
        }),
    );
    staged_app::seed(root);
    let resident = Resident::start(root);
    let lane = command_body(&spice_run(
        root,
        &["lane", "add", root.to_str().unwrap(), "--vendor", "claude"],
        None,
    ))["lane"]
        .as_str()
        .expect("lane add returns identity")
        .to_owned();
    command_body(&spice_run(root, &["lane", "wake", &lane], None));
    install_reference_hook(&scratch);
    let task = command_body(&spice_run(
        root,
        &[
            "task",
            "add",
            "Completion fixture",
            "--project",
            "task.git",
            "--acceptance",
            "the divergent agent commit lands without bypassing the reference hook",
            "--origin",
            "fixture:reference-transaction-cli",
        ],
        None,
    ))["task"]
        .as_str()
        .expect("task add returns handle")
        .to_owned();
    command_body(&spice_run(root, &["task", "capture", &task], Some(&lane)));
    git(root, &["update-ref", "refs/heads/main", &upstream, &base]);
    git(root, &["fetch", "origin"]);

    let done = spice_run(
        root,
        &["task", "done", "--validation", "fixture completion"],
        Some(&lane),
    );
    assert_success(&done, "task completion");
    let landing = git(root, &["rev-parse", "HEAD"]);
    assert_ne!(landing, agent);
    assert_eq!(
        git(root, &["rev-list", "--parents", "-n", "1", &landing]),
        format!("{landing} {upstream} {agent}")
    );
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    let shown = task_text(root, &task, &lane);
    assert!(shown.contains("phase review"), "{shown}");
    resident.stop(root);
}

fn exercise_hook_guarded_mismatch() {
    let fixture = Movement::new("hook-guarded-mismatch");
    publish_admission(
        fixture.scratch.path(),
        &fixture.tree,
        "refs/heads/main",
        None,
    );
    fixture.install_hook();
    let before = repository_git_state(fixture.scratch.path());
    let update = Command::new("/usr/bin/git")
        .args([
            "update-ref",
            "refs/heads/main",
            &fixture.candidate,
            &fixture.head,
        ])
        .current_dir(fixture.scratch.path())
        .output()
        .unwrap();
    assert_eq!(update.status.code(), Some(128), "{}", streams(&update));
    assert!(
        String::from_utf8_lossy(&update.stderr)
            .contains("gate-evidence::admission-candidate-mismatch"),
        "{}",
        streams(&update),
    );
    assert_eq!(repository_git_state(fixture.scratch.path()), before);
}

fn repository_git_state(root: &Path) -> Vec<String> {
    vec![
        git(root, &["rev-parse", "HEAD"]),
        git(root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        git(root, &["write-tree"]),
        git(root, &["status", "--porcelain"]),
    ]
}

fn completion_repository(label: &str) -> Scratch {
    let scratch = Scratch::repository(label);
    configure_identity(scratch.path());
    let skill = scratch.join(".git/.spice-install/agents/skills/spice/SKILL.md");
    fs::create_dir_all(skill.parent().expect("fixture skill parent"))
        .expect("fixture skill parent creates");
    fs::write(&skill, ::std::include_bytes!("../../agent/SKILL.md"))
        .expect("fixture installs appliance skill");
    scratch.executable(
        "fake-claude",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",",
            "\"session_id\":\"018f7777-7777-7444-8888-999999999999\"}'\n",
            "while [ -d .git ]; do sleep 1; done\n",
        )
        .as_bytes(),
    );
    fs::write(
        scratch.join("spice-rust.toml"),
        format!(
            "[tasks]\nstems = [\"task\"]\n\n[vendor.claude]\nbinary = {:?}\n",
            scratch.join("fake-claude").display().to_string()
        ),
    )
    .unwrap();
    fs::write(
        scratch.join("Cargo.toml"),
        "[package]\nname='completion-fixture'\nversion='0.0.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::write(
        scratch.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"completion-fixture\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    fs::write(scratch.join(".gitignore"), "dist/\nfixture-cargo-home/\n").unwrap();
    fs::write(scratch.join("tracked"), "base\n").unwrap();
    git(scratch.path(), &["add", "."]);
    git(scratch.path(), &["commit", "-m", "base"]);
    scratch
}

fn install_reference_hook(scratch: &Scratch) {
    scratch.executable(
        ".git/hooks/reference-transaction",
        format!(
            "#!/bin/sh\nexec '{}' gate reference-transaction \"$@\"\n",
            spice()
        )
        .as_bytes(),
    );
    git(
        scratch.path(),
        &[
            "config",
            "core.hooksPath",
            scratch.join(".git/hooks").to_str().unwrap(),
        ],
    );
}

fn command_body(output: &Output) -> serde_json::Value {
    assert_success(output, "spice command");
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["body"].clone()
}

fn task_text(root: &Path, task: &str, lane: &str) -> String {
    command_body(&spice_run(root, &["task", "show", task], Some(lane)))["text"]
        .as_str()
        .expect("task show returns text")
        .to_owned()
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
