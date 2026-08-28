use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wire::{Outcome, ReleaseNotes, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
#[path = "support/fixture_root.rs"]
mod fixture_root;

struct Fixture {
    root: PathBuf,
    serial: usize,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), &format!("release-notes-{name}"));
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.name", "Spice Test"]);
        git(&root, &["config", "user.email", "spice@example.test"]);
        Self { root, serial: 0 }
    }

    fn commit(&mut self, subject: &str, project: Option<&str>, task: Option<&str>) -> String {
        self.serial += 1;
        fs::write(
            self.root.join(format!("landing-{}.txt", self.serial)),
            format!("landing {}\n", self.serial),
        )
        .expect("landing fixture writes");
        git(&self.root, &["add", "."]);
        let mut message = subject.to_owned();
        if project.is_some() || task.is_some() {
            message.push_str("\n\n");
        }
        if let Some(project) = project {
            message.push_str(&format!("Task-Project: {project}\n"));
        }
        if let Some(task) = task {
            message.push_str(&format!("Task-Key: {task}\n"));
        }
        git(&self.root, &["commit", "-m", &message]);
        self.head()
    }

    fn legacy_commit(&mut self, subject: &str, project: &str, handle: &str) -> String {
        self.serial += 1;
        fs::write(
            self.root.join(format!("legacy-{}.txt", self.serial)),
            format!("legacy {}\n", self.serial),
        )
        .expect("legacy fixture writes");
        git(&self.root, &["add", "."]);
        let message = format!("{subject}\n\nTask-Project: {project}\nTask-Handle: {handle}\n");
        git(&self.root, &["commit", "-m", &message]);
        self.head()
    }

    fn merge(&mut self, branch: &str, subject: &str, project: &str, task: &str) -> String {
        let message = format!("{subject}\n\nTask-Project: {project}\nTask-Key: {task}\n");
        git(&self.root, &["merge", "--no-ff", branch, "-m", &message]);
        self.head()
    }

    fn head(&self) -> String {
        git(&self.root, &["rev-parse", "HEAD"])
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("spice release notes launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

#[test]
fn reachable_first_parent_history_becomes_one_stable_text_and_json_draft() {
    let mut fixture = Fixture::new("history");
    fixture.commit("repository root", None, None);
    git(&fixture.root, &["tag", "v1.0.0"]);

    git(&fixture.root, &["checkout", "-b", "unreachable"]);
    fixture.commit("future work", Some("future.work"), Some("future"));
    git(&fixture.root, &["tag", "v99.0.0"]);
    git(&fixture.root, &["checkout", "main"]);

    let first = fixture.commit(
        "canvas: Add palette UI-palette",
        Some("canvas.ui"),
        Some("palette"),
    );
    let second = fixture.commit(
        "canvas: Fix palette UI-palette (review)",
        Some("canvas.ui"),
        Some("palette"),
    );

    git(&fixture.root, &["checkout", "-b", "topic"]);
    fixture.commit("internal implementation one", None, None);
    fixture.commit("internal implementation two", None, None);
    git(&fixture.root, &["checkout", "main"]);
    let merge = fixture.merge(
        "topic",
        "dist: Add history notes NOTES-history",
        "dist.notes",
        "history",
    );
    git(&fixture.root, &["checkout", "-b", "doomed-topic"]);
    let doomed = fixture.commit("doomed side implementation", None, None);
    git(&fixture.root, &["checkout", "main"]);
    fixture.merge(
        "doomed-topic",
        "gate: Add doomed check GATE-doomed",
        "gate.tests",
        "doomed",
    );
    git(&fixture.root, &["revert", "--no-edit", &doomed]);
    fixture.commit("release: bump to 0.0.0", None, None);

    let before = git(&fixture.root, &["status", "--short"]);
    let text = fixture.invoke(&["release", "notes"]);
    assert!(text.status.success(), "{}", spoken(&text));
    let document = String::from_utf8(text.stdout.clone()).expect("text draft is UTF-8");
    assert!(document.contains("## Highlights"));
    assert!(document.contains("### Canvas UI"));
    assert!(document.contains("Fixed palette."));
    assert!(document.contains(&first[..7]));
    assert!(document.contains(&second[..7]));
    assert!(document.contains(&merge[..7]));
    assert!(!document.contains("internal implementation"));
    assert!(!document.contains("doomed"));
    assert!(!document.contains("future work"));
    assert!(!document.contains("release: bump"));
    assert!(document.contains("refs/tags/v1.0.0.."));

    let json = fixture.invoke(&["--json", "release", "notes"]);
    assert!(json.status.success(), "{}", spoken(&json));
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&json.stdout).expect("JSON draft is typed")
    else {
        panic!("release notes did not reply")
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body.previous_tag.as_deref(), Some("v1.0.0"));
    assert!(!reply.body.bounded_recent_window);
    assert_eq!(reply.body.entries.len(), 2);
    assert_eq!(reply.body.entries[0].references, [first, second]);
    assert_eq!(reply.body.document, document);
    assert_eq!(git(&fixture.root, &["status", "--short"]), before);
}

#[test]
fn an_explicit_release_commit_and_no_tag_fallback_are_both_visible() {
    let mut fixture = Fixture::new("bounded");
    let mut commits = Vec::new();
    for index in 0..7 {
        commits.push(fixture.commit(
            &format!("core: Add change {index} TASK-{index}"),
            Some("core"),
            Some(&index.to_string()),
        ));
    }

    let explicit = fixture.invoke(&[
        "--json",
        "release",
        "notes",
        "--release-commit",
        &commits[4],
    ]);
    assert!(explicit.status.success(), "{}", spoken(&explicit));
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&explicit.stdout).expect("draft is typed")
    else {
        panic!("release notes did not reply")
    };
    assert_eq!(reply.body.release_commit, commits[4]);
    assert!(reply.body.bounded_recent_window);
    assert_eq!(reply.body.entries.len(), 5);
    assert!(
        reply
            .body
            .document
            .contains("bounded latest first-parent commits")
    );
}

#[test]
fn legacy_task_handle_landings_are_normalized_to_task_key() {
    let mut fixture = Fixture::new("legacy-task-handle");
    fixture.commit("repository root", None, None);
    git(&fixture.root, &["tag", "v1.0.0"]);
    let landing = fixture.legacy_commit(
        "core: Add legacy trailer cleanup CORE-legacy",
        "core.git",
        "CORE-legacy",
    );

    let output = fixture.invoke(&["--json", "release", "notes"]);
    assert!(output.status.success(), "{}", spoken(&output));
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&output.stdout).expect("JSON draft is typed")
    else {
        panic!("release notes did not reply")
    };

    assert_eq!(reply.body.entries.len(), 1);
    assert_eq!(reply.body.entries[0].task.as_deref(), Some("legacy"));
    assert_eq!(reply.body.entries[0].references, [landing]);
    assert!(
        reply
            .body
            .document
            .contains("Added legacy trailer cleanup.")
    );
    assert!(!reply.body.document.contains("CORE-legacy"));
}

#[test]
fn reverting_a_target_before_the_selected_tag_keeps_every_window_landing() {
    let mut fixture = Fixture::new("pre-tag-revert");
    fixture.commit("repository root", None, None);
    let target = fixture.commit(
        "core: Add pre-tag behavior CORE-before",
        Some("core.behavior"),
        Some("before"),
    );
    git(&fixture.root, &["tag", "v1.0.0"]);

    let unrelated = fixture.commit(
        "canvas: Add unrelated palette UI-unrelated",
        Some("canvas.ui"),
        Some("unrelated"),
    );
    git(&fixture.root, &["revert", "--no-edit", &target]);
    let revert = fixture.head();

    let output = fixture.invoke(&["--json", "release", "notes"]);
    assert!(output.status.success(), "{}", spoken(&output));
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<ReleaseNotes>>(&output.stdout).expect("JSON draft is typed")
    else {
        panic!("release notes did not reply")
    };

    assert_eq!(reply.body.previous_tag.as_deref(), Some("v1.0.0"));
    assert_eq!(reply.body.entries.len(), 2);
    assert_eq!(reply.body.entries[0].references, [unrelated]);
    assert_eq!(reply.body.entries[1].references, [revert]);
    assert!(reply.body.document.contains("unrelated palette"));
    assert!(reply.body.document.contains("Revert"));
}

#[test]
fn an_unknown_release_commit_is_one_typed_refusal() {
    let mut fixture = Fixture::new("unknown");
    fixture.commit("root", None, None);

    let output = fixture.invoke(&[
        "--json",
        "release",
        "notes",
        "--release-commit",
        "not-a-commit",
    ]);
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::<ReleaseNotes>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("refusal is typed")
    else {
        panic!("unknown release commit did not refuse")
    };
    assert_eq!(refusal.reason(), "release::history-refused");
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git launches");
    assert!(output.status.success(), "{}", spoken(&output));
    String::from_utf8(output.stdout)
        .expect("Git stdout is UTF-8")
        .trim()
        .to_owned()
}

fn spoken(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
