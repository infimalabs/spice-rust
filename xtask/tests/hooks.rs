#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{InstallStatus, apply, installed_git_hooks, plan};
use spice::repository::Repository;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const ACTIVE_HOOKS: [&str; 3] = ["pre-commit", "commit-msg", "reference-transaction"];
const GIT_DIRECTORY: &str = concat!("GIT_", "DIR");
const GIT_WORKING_TREE: &str = concat!("GIT_", "WORK_TREE");
const GIT_INDEX: &str = concat!("GIT_", "INDEX_FILE");
const VALID_ORIGIN: &str =
    "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=HOOKS-1kJQnmG8";
type FileImage = (Vec<u8>, u32);

#[test]
fn install_exercises_spice_hooks_before_explicit_activation() {
    let scratch = Scratch::new();
    let (base, protected, local) = repository_history(&scratch);
    let installation = install_spice_hooks(&scratch);
    let cargo_log = scratch.join("cargo.log");
    install_cargo_alias(&scratch, &cargo_log);

    exercise_pre_commit(&scratch, &cargo_log);
    exercise_commit_message(&scratch);
    assert_prior_hooks_unchanged(&scratch, &installation);
    exercise_reference_guard(&scratch, &base, &protected, &local);
    // The guard ran through the installed hook several times above, holding the
    // packed-refs lock each time. The Cargo alias appends every invocation it
    // is handed, so a log naming no reference transaction is the runtime half
    // of the structural pin in `tracked_current_hook`: the hook reached the
    // published executable, and the build system was never entered at all.
    let invocations = fs::read_to_string(&cargo_log).unwrap();
    assert!(
        !invocations.contains("reference-transaction"),
        "the reference-transaction hook must not reach Cargo: {invocations}"
    );
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&installation.operator.join(hook))),
        installation.operator_hooks
    );
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&installation.tracked.join(hook))),
        installation.tracked_hooks
    );
}

#[test]
fn activation_refuses_predecessor_hooks_before_any_publication_or_config_change() {
    let scratch = Scratch::new();
    repository_history(&scratch);
    let installation = install_spice_hooks(&scratch);
    git(&scratch, &["config", "extensions.worktreeConfig", "true"]);
    git(
        &scratch,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "predecessor",
            "predecessor",
            "HEAD",
        ],
    );
    let predecessor = scratch.join("predecessor");
    for hook in ACTIVE_HOOKS {
        let path = predecessor.join(".spice/hooks").join(hook);
        fs::create_dir_all(path.parent().unwrap()).expect("create predecessor hook directory");
        fs::write(&path, format!("#!/bin/sh\nexec spice dev {hook} \"$@\"\n"))
            .expect("write predecessor hook");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make predecessor hook executable");
    }
    let configured = Command::new("git")
        .args(["config", "--worktree", "core.hooksPath", ".spice/hooks"])
        .current_dir(&predecessor)
        .output()
        .expect("configure predecessor hook path");
    assert_success(&configured, "configure predecessor hook path");

    let repository_config = image(&scratch.join(".git/config"));
    let worktree_config = image(&scratch.join(".git/worktrees/predecessor/config.worktree"));
    let published = published_entry_point(scratch.path());
    let published_before = image(&published);
    let replacement = scratch.executable("replacement-xtask", b"#!/bin/sh\nexit 0\n");
    let mut output = Vec::new();

    let error = xtask::activate_hooks(scratch.path(), &replacement, &mut output)
        .expect_err("foreign predecessor hooks must refuse activation");

    assert!(
        matches!(error, xtask::HookActivationError::ForeignHookPath { .. }),
        "{error}"
    );
    assert!(error.to_string().contains(".spice/hooks"), "{error}");
    assert!(
        error
            .to_string()
            .contains(&predecessor.display().to_string()),
        "{error}"
    );
    assert!(output.is_empty(), "refusal published output: {output:?}");
    assert_eq!(image(&scratch.join(".git/config")), repository_config);
    assert_eq!(
        image(&scratch.join(".git/worktrees/predecessor/config.worktree")),
        worktree_config
    );
    assert_eq!(image(&published), published_before);
    assert_eq!(configured_hooks(&scratch), ".githooks");
    assert_eq!(configured_hooks_at(&predecessor), ".spice/hooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&installation.operator.join(hook))),
        installation.operator_hooks
    );
}

#[test]
fn activation_admits_repeated_identical_tolerated_hook_values() {
    for select_target in [false, true] {
        let scratch = Scratch::new();
        repository_history(&scratch);
        install_spice_hooks(&scratch);
        git(&scratch, &["config", "extensions.worktreeConfig", "true"]);
        let target = common_hooks(&scratch).display().to_string();
        let selection = if select_target { &target } else { ".githooks" };
        git(
            &scratch,
            &["config", "--local", "core.hooksPath", selection],
        );
        git(
            &scratch,
            &["config", "--worktree", "core.hooksPath", selection],
        );
        assert_eq!(
            git_at(scratch.path(), &["config", "--get-all", "core.hooksPath"])
                .lines()
                .collect::<Vec<_>>(),
            [selection, selection]
        );

        let replacement = scratch.executable("replacement-xtask", b"#!/bin/sh\nexit 0\n");
        let mut output = Vec::new();
        xtask::activate_hooks(scratch.path(), &replacement, &mut output)
            .expect("identical tolerated selections admit activation");

        assert_eq!(configured_hooks(&scratch), target);
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("cleared worktree hook override")
        );
    }
}

#[test]
fn activation_refuses_mixed_tolerated_and_repeated_foreign_hook_values() {
    for repeat_foreign in [false, true] {
        let scratch = Scratch::new();
        repository_history(&scratch);
        install_spice_hooks(&scratch);
        git(&scratch, &["config", "extensions.worktreeConfig", "true"]);
        let target = common_hooks(&scratch).display().to_string();
        let (local, worktree) = if repeat_foreign {
            (".spice/hooks", ".spice/hooks")
        } else {
            (".githooks", target.as_str())
        };
        git(&scratch, &["config", "--local", "core.hooksPath", local]);
        git(
            &scratch,
            &["config", "--worktree", "core.hooksPath", worktree],
        );
        assert_eq!(
            git_at(scratch.path(), &["config", "--get-all", "core.hooksPath"])
                .lines()
                .collect::<Vec<_>>(),
            [local, worktree]
        );
        let repository_config = image(&scratch.join(".git/config"));
        let worktree_config = image(&scratch.join(".git/config.worktree"));
        let published = published_entry_point(scratch.path());
        let published_before = image(&published);
        let replacement = scratch.executable("replacement-xtask", b"#!/bin/sh\nexit 0\n");
        let mut output = Vec::new();

        let error = xtask::activate_hooks(scratch.path(), &replacement, &mut output)
            .expect_err("more than one selection or a foreign selection must refuse");

        assert!(matches!(
            error,
            xtask::HookActivationError::ForeignHookPath { .. }
        ));
        assert!(output.is_empty());
        assert_eq!(image(&scratch.join(".git/config")), repository_config);
        assert_eq!(
            image(&scratch.join(".git/config.worktree")),
            worktree_config
        );
        assert_eq!(image(&published), published_before);
    }
}

/// A lane running under the git shadow environment sees its own branch as its
/// upstream, deliberately, so `git status` never moves when origin advances.
/// The reference guard must read the integration branch the shadow keeps
/// readable instead, or every backwards move of an unpublished lane commit is
/// reported as abandoning published history.
#[test]
fn a_shadowed_upstream_does_not_publish_a_lanes_local_commits() {
    let scratch = Scratch::new();
    let (base, protected, local) = repository_history(&scratch);
    shadow_lane_branch(&scratch, "main-b", &local, &base);

    assert_eq!(rev_parse(&scratch, "@{upstream}^{commit}"), local);
    assert_eq!(
        config_get(&scratch, "branch.main-b.merge"),
        "refs/heads/main"
    );
    // The trap this test exists for: the effective remote is the shadow's `.`
    // while the repository's own pairing still names origin. Without this the
    // fixture could stop reproducing the disagreement and the test below would
    // keep passing for the wrong reason.
    assert_eq!(config_get(&scratch, "branch.main-b.remote"), ".");
    assert_eq!(config_get_local(&scratch, "branch.main-b.remote"), "origin");
    assert_eq!(rev_parse(&scratch, "refs/remotes/origin/main"), protected);
    assert_eq!(rev_parse(&scratch, "refs/heads/main"), base);

    guard(
        &scratch,
        &format!("{local} {protected} refs/heads/main-b\n"),
    )
    .expect("a shadowed lane may rewind a commit it never published");

    let refusal = guard(&scratch, &format!("{protected} {base} refs/heads/main-b\n"))
        .expect_err("history published to the integration branch stays protected");
    assert!(
        refusal
            .to_string()
            .contains("refused to abandon upstream-merged commits")
    );
}

/// What Git writes in the old field of a queue line when nothing was declared.
///
/// `git update-ref <reference> <new>` takes an optional old value and declares
/// none, so the field carries the zero OID whether the update rewinds published
/// history or creates a branch. Spelled here rather than imported: the guard's
/// own constant agreeing with itself would prove nothing about what Git sends.
const UNDECLARED_OLD: &str = "0000000000000000000000000000000000000000";

/// The tip is read from the reference, never from the queue's old field.
///
/// The field is whatever the caller declared, and the caller Git runs for
/// `update-ref` declares nothing. A guard judging it reads every such rewind as
/// a branch creation and refuses none of them -- which is the branch most likely
/// to be moved that way left unprotected against the command most likely to move
/// it.
#[test]
fn a_rewind_declaring_no_old_value_is_judged_against_the_reference() {
    let scratch = Scratch::new();
    let (base, protected, _local) = repository_history(&scratch);

    // The fixture's premise, stated so it cannot quietly stop holding: the
    // reference carries published history while the queue says it carries
    // nothing. If these ever agreed, the zero below would be the truth and
    // permitting it would be correct.
    assert_eq!(reference_tip(&scratch, "refs/heads/main"), protected);
    assert_eq!(rev_parse(&scratch, "@{upstream}^{commit}"), protected);

    let refusal = guard(
        &scratch,
        &format!("{UNDECLARED_OLD} {base} refs/heads/main\n"),
    )
    .expect_err("a rewind is judged against the tip the reference holds");
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("refused to abandon upstream-merged commits"),
        "{refusal}"
    );
    assert!(
        refusal.contains(&protected[..7]),
        "the refusal must name the commit the walk found, so a refusal for some \
         other reason cannot stand in for one that started at the real tip: \
         {refusal}"
    );
}

/// A creation stays permitted, and is permitted only for having no tip.
///
/// Reading the reference must not turn every new branch into a refusal: an
/// unborn branch has nothing to abandon. The branch is created *behind* the
/// published baseline rather than at it, which is what `git checkout -b` off an
/// older commit does and what makes this arm able to fail. Created at the
/// baseline, a fix that substituted some other tip -- the baseline itself being
/// the obvious one -- would still permit it as a fast-forward, and the test
/// would pass without proving the absent tip was what permitted it.
///
/// The second half is the other control. It gives that same branch a tip and
/// re-sends a queue line identical but for the commit; the refusal proves the
/// permit above came from the absent tip rather than from the guard falling
/// inert earlier -- an unpaired branch or an unmatched reference would also have
/// returned quietly.
#[test]
fn creating_the_current_branch_is_permitted_only_while_it_has_no_tip() {
    let scratch = Scratch::new();
    let (base, protected, _local) = repository_history(&scratch);
    let branch = "main-created";
    let unborn = format!("refs/heads/{branch}");
    git(&scratch, &["symbolic-ref", "HEAD", &unborn]);
    // Paired, so the guard reaches the judgement rather than falling inert at
    // the baseline and permitting the creation below for the wrong reason.
    let remote = format!("branch.{branch}.remote");
    let merge = format!("branch.{branch}.merge");
    git(&scratch, &["config", &remote, "origin"]);
    git(&scratch, &["config", &merge, "refs/heads/main"]);

    assert_eq!(
        reference_tip(&scratch, &unborn),
        "",
        "the branch already exists, so this is a rewind rather than a creation"
    );
    guard(&scratch, &format!("{UNDECLARED_OLD} {base} {unborn}\n"))
        .expect("a branch that holds no tip abandons nothing by being created");

    git(&scratch, &["update-ref", &unborn, &protected]);
    assert_eq!(reference_tip(&scratch, &unborn), protected);
    let refusal = guard(&scratch, &format!("{UNDECLARED_OLD} {base} {unborn}\n"))
        .expect_err("the same branch, now holding published history, is judged");
    assert!(
        refusal
            .to_string()
            .contains("refused to abandon upstream-merged commits"),
        "{refusal}"
    );
}

/// The commit a reference holds, or the empty string if it holds none.
///
/// `rev_parse` cannot answer for an unborn branch: it fails, and the fixture
/// helper turns a failure into a panic rather than an answer.
fn reference_tip(scratch: &Scratch, reference: &str) -> String {
    let arguments = ["for-each-ref", "--format=%(objectname)", reference];
    String::from_utf8(git(scratch, &arguments).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

/// Put `lane` under the shadow, in the scopes the real shadow writes.
///
/// Measured in a live lane worktree with `git config --show-origin` rather than
/// reconstructed: `branch.<lane>.merge` carries `refs/heads/<lane>` from the
/// agent's generated config and `refs/heads/main` from the repository's own,
/// while `branch.<lane>.remote` carries `origin` from the repository and `.`
/// from the command line. So the shadow never touches the repository's real
/// pairing; it layers over it. `@{upstream}` resolves to the lane itself
/// because git tracking reads the *first* merge value and the single-valued
/// remote resolves to `.`, and that is the whole point of the shadow --
/// `git status` stops moving when origin advances.
///
/// The two halves are therefore recorded differently on purpose. The self-merge
/// is added first and the true integration branch last, reproducing the order
/// that makes `@{upstream}` self-referential while the repository's own value
/// stays last. The `.` remote goes to worktree scope, which stands in for the
/// command scope a fixture cannot set: both outrank local, and what matters is
/// that the effective remote disagrees with the local one. A guard that read
/// the effective value would find `.` and quietly protect nothing.
///
/// The local `refs/heads/main` is left behind at `stale` so it cannot stand in
/// for the remote-tracking reference the contract names; a lane worktree has no
/// reason to hold a current copy of the integration branch it never checks out.
fn shadow_lane_branch(scratch: &Scratch, lane: &str, tip: &str, stale: &str) {
    git(scratch, &["branch", lane, tip]);
    git(
        scratch,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{lane}")],
    );
    git(scratch, &["update-ref", "refs/heads/main", stale]);
    let merge = format!("branch.{lane}.merge");
    let remote = format!("branch.{lane}.remote");
    git(
        scratch,
        &["config", "--add", &merge, &format!("refs/heads/{lane}")],
    );
    git(scratch, &["config", "--local", &remote, "origin"]);
    git(scratch, &["config", "--add", &merge, "refs/heads/main"]);
    git(scratch, &["config", "extensions.worktreeConfig", "true"]);
    git(scratch, &["config", "--worktree", &remote, "."]);
}

fn guard(scratch: &Scratch, prepared: &str) -> Result<(), impl std::error::Error + use<>> {
    xtask::reference_transaction(scratch.path(), OsStr::new("prepared"), prepared)
}

fn config_get(scratch: &Scratch, key: &str) -> String {
    String::from_utf8(git(scratch, &["config", "--get", key]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

/// The repository's own value, ignoring whatever the shadow layers above it.
fn config_get_local(scratch: &Scratch, key: &str) -> String {
    String::from_utf8(git(scratch, &["config", "--local", "--get", key]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

fn install_spice_hooks(scratch: &Scratch) -> PriorHookEvidence {
    let operator = scratch.join(".spice/hooks");
    let tracked = scratch.join(".githooks");
    fs::create_dir_all(&operator).expect("create operator hook directory");
    for hook in ACTIVE_HOOKS {
        scratch.executable(
            &format!(".spice/hooks/{hook}"),
            format!("#!/bin/sh\nexec spice dev {hook} \"$@\"\n").as_bytes(),
        );
        scratch.executable(&format!(".githooks/{hook}"), &tracked_current_hook(hook));
    }
    // The tracked hooks go live here, and the reference-transaction hook among
    // them execs the published entry point on every ref update from now on. An
    // absent entry point aborts the transaction rather than falling back to
    // anything, so a repository with these hooks configured is by construction
    // one that has been activated before; the fixture publishes the entry point
    // to be that repository instead of a state the hook is meant to refuse.
    publish_fixture_entry_point(scratch);
    git(
        scratch,
        &["config", "--local", "core.hooksPath", ".githooks"],
    );
    let operator_before = ACTIVE_HOOKS.map(|hook| image(&operator.join(hook)));
    let tracked_before = ACTIVE_HOOKS.map(|hook| image(&tracked.join(hook)));
    let config_before = image(&scratch.join(".git/config"));
    let repository = Repository::discover(scratch.path()).expect("discover temporary repository");
    let planned = plan(&repository, None).expect("plan Spice hooks");
    let receipt = apply(&repository, None, &planned.digest().expect("digest plan"))
        .expect("install Spice hooks");
    assert_eq!(receipt.status(), InstallStatus::Complete);

    let installed_hooks = installed_git_hooks(&repository);
    let pre_commit = installed_hooks.join("pre-commit");
    let commit_message = installed_hooks.join("commit-msg");
    let reference_transaction = installed_hooks.join("reference-transaction");
    assert_eq!(mode(&pre_commit), 0o755);
    assert_eq!(mode(&commit_message), 0o755);
    assert_eq!(mode(&reference_transaction), 0o755);
    assert_eq!(configured_hooks(scratch), ".githooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&operator.join(hook))),
        operator_before
    );
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&tracked.join(hook))),
        tracked_before
    );
    assert_eq!(image(&scratch.join(".git/config")), config_before);
    PriorHookEvidence {
        operator,
        operator_hooks: operator_before,
        tracked,
        tracked_hooks: tracked_before,
        config: config_before,
    }
}

fn tracked_current_hook(hook: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join(".githooks").join(hook);
    let bytes = fs::read(&path).expect("read tracked native hook wrapper");
    let installed = fs::read(root.join("spice/src/install/git-hooks").join(hook))
        .expect("read installed native hook template");
    assert_eq!(
        bytes, installed,
        "tracked and installed hooks must be one image"
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("spice dev"));
    if hook == "pre-commit" {
        assert_eq!(
            text.matches("cargo --quiet xtask verify-action battery")
                .count(),
            1
        );
        assert!(!text.contains("cargo xtask verify"));
    }
    if hook == "reference-transaction" {
        // Git runs this one inside the transaction, holding the repository
        // global packed-refs lock, so a build system in its path rebuilds under
        // a lock every worktree contends for. The pin is the absence of the
        // word rather than a bound on how long the hook takes: a machine with a
        // warm build directory hides the cost entirely, so timing cannot notice
        // the wrapper coming back and reading the bytes always can.
        assert!(
            !text.contains("cargo"),
            "the reference-transaction hook must reach a built executable \
             with no build system anywhere in its path: {text}"
        );
        assert_eq!(text.matches("exec ").count(), 1);
        assert!(text.contains("/.spice-install/bin/xtask\" reference-transaction"));
    }
    bytes
}

fn exercise_pre_commit(scratch: &Scratch, cargo_log: &Path) {
    let pre_commit = common_hooks(scratch).join("pre-commit");
    let pre_commit_output = Command::new(&pre_commit)
        .current_dir(scratch.path())
        .env(GIT_DIRECTORY, scratch.join(".git"))
        .env(GIT_WORKING_TREE, scratch.path())
        .env(GIT_INDEX, scratch.join(".git/index"))
        .output()
        .expect("run pre-commit with Git's local environment exported");
    assert_success(&pre_commit_output, "pre-commit");
    assert!(pre_commit_output.stdout.is_empty());
    assert!(pre_commit_output.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(cargo_log).unwrap(),
        "verify-action battery\n"
    );
}

fn exercise_commit_message(scratch: &Scratch) {
    let commit_message = common_hooks(scratch).join("commit-msg");
    let message = scratch.join("COMMIT_EDITMSG");
    fs::write(
        &message,
        format!(
            "Preserve native hook policy\n\n{}\n\n{VALID_ORIGIN}\n",
            "body prose ".repeat(14)
        ),
    )
    .expect("write valid commit message");
    let valid = invoke(&commit_message, scratch.path(), &[message.as_os_str()]);
    assert!(valid.status.success(), "{}", streams(&valid));
    assert!(valid.stdout.is_empty());
    assert!(
        String::from_utf8(valid.stderr)
            .unwrap()
            .contains("auto-folded commit message body lines")
    );
    assert!(
        fs::read_to_string(&message)
            .unwrap()
            .lines()
            .all(|line| line.chars().count() <= xtask::COMMIT_MESSAGE_WRAP_LIMIT)
    );

    fs::write(
        &message,
        "Preserve native provenance\n\nSpice-Origin: v2 unresolved\n",
    )
    .expect("write malformed provenance");
    let malformed = invoke(&commit_message, scratch.path(), &[message.as_os_str()]);
    assert_eq!(malformed.status.code(), Some(1));
    assert!(malformed.stdout.is_empty());
    let diagnostic = String::from_utf8(malformed.stderr).unwrap();
    assert!(diagnostic.contains("malformed Spice-Origin trailer"));
    assert!(diagnostic.contains("Spice-Origin: v2 unresolved"));

    fs::write(
        scratch.join("spice-rust.toml"),
        "[policy.commit_message]\nblocked_trailers = [\"Co-Authored-By\"]\n",
    )
    .expect("write tracked trailer policy");
    fs::write(
        &message,
        "Preserve native attribution\n\nCo-Authored-By: Agent <agent@example.test>\n",
    )
    .expect("write policy-refused attribution");
    let blocked = invoke(&commit_message, scratch.path(), &[message.as_os_str()]);
    assert_eq!(blocked.status.code(), Some(1));
    assert!(blocked.stdout.is_empty());
    let diagnostic = String::from_utf8(blocked.stderr).unwrap();
    assert!(diagnostic.contains("uses blocked trailer co-authored-by"));
    assert!(diagnostic.contains("blocked trailers: co-authored-by"));
}

fn assert_prior_hooks_unchanged(scratch: &Scratch, evidence: &PriorHookEvidence) {
    assert_eq!(configured_hooks(scratch), ".githooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&evidence.operator.join(hook))),
        evidence.operator_hooks
    );
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&evidence.tracked.join(hook))),
        evidence.tracked_hooks
    );
    assert_eq!(image(&scratch.join(".git/config")), evidence.config);
}

fn exercise_reference_guard(scratch: &Scratch, base: &str, protected: &str, local: &str) {
    assert_eq!(rev_parse(scratch, "@{upstream}^{commit}"), protected);
    let prepared_input = format!("{protected} {base} refs/heads/main\n");
    let direct_refusal =
        xtask::reference_transaction(scratch.path(), OsStr::new("prepared"), &prepared_input)
            .expect_err("direct native guard refuses protected rewind");
    assert!(
        direct_refusal
            .to_string()
            .contains("refused to abandon upstream-merged commits")
    );

    let linked = scratch.join("linked");
    git(
        scratch,
        &["worktree", "add", "--quiet", "--detach", "linked", "HEAD"],
    );
    // Before activation nothing execs the published copy, so a stale one
    // enforces nothing and the audit owes no refusal. It has to say that rather
    // than pass quietly, or an inert repository and a current one look alike.
    let mut inert = Vec::new();
    xtask::audit_entry_point(scratch.path(), &mut inert)
        .expect("an unactivated repository owes no guard refusal");
    let inert = String::from_utf8(inert).unwrap();
    assert!(
        inert.contains("not activated"),
        "the audit names the inert case: {inert}"
    );

    let mut activated = Vec::new();
    // The binary Cargo built for this test, not `current_exe`: in an
    // integration test that would be the harness, and publishing the harness
    // would leave the hook running something with no guard in it at all.
    let entry_point = Path::new(env!("CARGO_BIN_EXE_xtask"));
    xtask::activate_hooks(scratch.path(), entry_point, &mut activated)
        .expect("run explicit hook activation");
    assert_entry_point_answers_the_guard(scratch, entry_point, &prepared_input);
    // One executable serves every worktree, so publishing to a path they share
    // is the answer rather than a compromise. The linked worktree resolves the
    // same file, which is also what makes a second lane activating over a first
    // lane's copy uninteresting: `reference_transaction::apply` reads only the
    // repository at the current directory, through Git, and carries no lane
    // identity for a per-lane copy to differ on.
    assert_eq!(
        published_entry_point(&linked),
        published_entry_point(scratch.path())
    );
    assert_stale_entry_point_is_republished(scratch, entry_point);
    assert_unusable_entry_point_is_repaired(scratch, entry_point);
    assert_guard_audit_refuses_a_prior_build(scratch, entry_point);
    assert_an_absent_entry_point_aborts_ref_updates(scratch, entry_point);
    let shared_hooks = common_hooks(scratch);
    assert_eq!(
        configured_hooks(scratch),
        shared_hooks.display().to_string()
    );
    assert_eq!(
        configured_hooks_at(&linked),
        shared_hooks.display().to_string()
    );
    assert_eq!(
        git_at(&linked, &["rev-parse", "--git-path", "hooks"]),
        shared_hooks.display().to_string()
    );
    assert!(!linked.join(".spice-install").exists());
    assert!(
        String::from_utf8(activated)
            .unwrap()
            .contains("activated Spice hooks")
    );

    let refused = update_ref(scratch, base, protected);
    assert_ne!(refused.status.code(), Some(0), "{}", streams(&refused));
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("refused to abandon upstream-merged commits")
    );
    assert_eq!(rev_parse(scratch, "refs/heads/main"), protected);

    assert_success(
        &update_ref(scratch, local, protected),
        "advance current branch to local commit",
    );
    assert_success(
        &update_ref(scratch, protected, local),
        "rewind only the unmerged local commit",
    );
    assert_eq!(rev_parse(scratch, "refs/heads/main"), protected);
}

struct PriorHookEvidence {
    operator: PathBuf,
    operator_hooks: [FileImage; 3],
    tracked: PathBuf,
    tracked_hooks: [FileImage; 3],
    config: FileImage,
}

fn install_cargo_alias(scratch: &Scratch, log: &Path) {
    let xtask = Path::new(env!("CARGO_BIN_EXE_xtask"));
    assert!(!xtask.as_os_str().is_empty());
    assert!(!xtask.to_string_lossy().contains('\''));
    assert!(!log.to_string_lossy().contains('\''));
    let dispatcher = scratch.executable(
        "bin/xtask-dispatcher",
        format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "for variable in '{}' '{}' '{}'; do\n",
                "    eval \"value=\\${{$variable-}}\"\n",
                "    if [ -n \"$value\" ]; then\n",
                "        printf 'pre-commit leaked local Git environment: ",
                "%s\\n' \"$variable\" >&2\n",
                "        exit 65\n",
                "    fi\n",
                "done\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "if [ \"$#\" -eq 2 ] && [ \"$1\" = verify-action ] && ",
                "[ \"$2\" = battery ]; then\n",
                "    exit 0\n",
                "fi\n",
                "if [ \"$#\" -eq 2 ] && [ \"$1\" = commit-msg ]; then\n",
                "    exec '{}' commit-msg \"$2\"\n",
                "fi\n",
                // No reference-transaction arm on purpose. The hook execs the
                // published binary directly now, so routing that subcommand
                // back through Cargo has to fail loudly here rather than be
                // quietly served: an arm answering it would let the deleted
                // wrapper return and every assertion below still pass.
                "printf 'unexpected xtask invocation: %s\\n' \"$*\" >&2\n",
                "exit 64\n",
            ),
            GIT_DIRECTORY,
            GIT_WORKING_TREE,
            GIT_INDEX,
            log.display(),
            xtask.display(),
        )
        .as_bytes(),
    );
    assert!(!dispatcher.to_string_lossy().contains('"'));
    assert!(!dispatcher.to_string_lossy().contains('\\'));
    let alias_package = scratch.join("bin/xtask-alias");
    fs::create_dir_all(alias_package.join("src")).expect("create Cargo alias package");
    fs::write(
        alias_package.join("Cargo.toml"),
        "[package]\nname = \"hook-xtask-alias\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("write Cargo alias manifest");
    fs::write(
        alias_package.join("src/main.rs"),
        format!(
            concat!(
                "fn main() {{\n",
                "    let status = std::process::Command::new(\"{}\")\n",
                "        .args(std::env::args_os().skip(1))\n",
                "        .status()\n",
                "        .expect(\"run hook xtask dispatcher\");\n",
                "    std::process::exit(status.code().unwrap_or(1));\n",
                "}}\n",
            ),
            dispatcher.display(),
        ),
    )
    .expect("write Cargo alias binary");
    fs::create_dir_all(scratch.join(".cargo")).expect("create Cargo alias directory");
    fs::write(
        scratch.join(".cargo/config.toml"),
        format!(
            concat!(
                "[alias]\n",
                "xtask = [\"run\", \"--quiet\", \"--manifest-path\", \"{}\", \"--\"]\n",
            ),
            alias_package.join("Cargo.toml").display(),
        ),
    )
    .expect("write Cargo alias");
}

fn invoke(program: &Path, root: &Path, arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(program)
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run installed hook")
}

fn repository_history(scratch: &Scratch) -> (String, String, String) {
    git(scratch, &["config", "user.name", "Spice Hook Test"]);
    git(scratch, &["config", "user.email", "hooks@example.invalid"]);

    let story = scratch.join("story.txt");
    fs::write(&story, "base\n").expect("write base story");
    git(scratch, &["add", "story.txt"]);
    git(scratch, &["commit", "--quiet", "-m", "Create base history"]);
    let base = rev_parse(scratch, "HEAD");

    fs::write(&story, "base\nprotected\n").expect("write protected story");
    git(
        scratch,
        &["commit", "--quiet", "-am", "Publish protected history"],
    );
    let protected = rev_parse(scratch, "HEAD");

    fs::write(&story, "base\nprotected\nlocal\n").expect("write local story");
    git(scratch, &["commit", "--quiet", "-am", "Keep local history"]);
    let local = rev_parse(scratch, "HEAD");

    git(scratch, &["remote", "add", "origin", "."]);
    git(
        scratch,
        &["update-ref", "refs/remotes/origin/main", &protected],
    );
    git(
        scratch,
        &["update-ref", "refs/heads/main", &protected, &local],
    );
    git(scratch, &["config", "branch.main.remote", "origin"]);
    git(scratch, &["config", "branch.main.merge", "refs/heads/main"]);
    (base, protected, local)
}

fn update_ref(scratch: &Scratch, new: &str, old: &str) -> Output {
    Command::new("git")
        .args(["update-ref", "refs/heads/main", new, old])
        .current_dir(scratch.path())
        .output()
        .expect("update current branch reference")
}

fn rev_parse(scratch: &Scratch, revision: &str) -> String {
    String::from_utf8(git(scratch, &["rev-parse", revision]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

fn configured_hooks(scratch: &Scratch) -> String {
    configured_hooks_at(scratch.path())
}

fn configured_hooks_at(root: &Path) -> String {
    git_at(root, &["config", "--get", "core.hooksPath"])
}

fn common_hooks(scratch: &Scratch) -> PathBuf {
    let repository = Repository::discover(scratch.path()).expect("discover hook repository");
    installed_git_hooks(&repository)
}

fn published_entry_point(root: &Path) -> PathBuf {
    let repository = Repository::discover(root).expect("discover hook repository");
    repository.common_dir().join(".spice-install/bin/xtask")
}

/// Put the entry point in place the way an earlier activation would have.
///
/// This copies rather than calling activation, because activation also flips
/// `core.hooksPath`, and that flip is the one thing this test watches for
/// before and after. Borrowing it here would consume the transition it exists
/// to observe.
fn publish_fixture_entry_point(scratch: &Scratch) {
    let published = published_entry_point(scratch.path());
    fs::create_dir_all(published.parent().expect("entry point has a parent"))
        .expect("create fixture entry point directory");
    fs::copy(env!("CARGO_BIN_EXE_xtask"), &published).expect("publish fixture entry point");
    fs::set_permissions(&published, fs::Permissions::from_mode(0o755))
        .expect("make fixture entry point executable");
}

/// A stale copy is noticed by activation and by nothing else, so this drives
/// the only path that can notice one.
///
/// Nothing rebuilds at hook time, by design, so the published copy stays
/// exactly as current as the last activation left it and no amount of running
/// the hook will reveal that it is behind. Overwriting it here stands in for
/// the ordinary case of a copy left by an older build: to activation both are
/// simply bytes that differ from the ones it is holding, which is the entire
/// comparison it makes.
fn assert_stale_entry_point_is_republished(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::write(&published, b"#!/bin/sh\nexit 0\n").expect("stale the published entry point");
    let mut reactivated = Vec::new();
    xtask::activate_hooks(scratch.path(), source, &mut reactivated)
        .expect("re-run activation over a stale entry point");
    let reactivated = String::from_utf8(reactivated).unwrap();
    assert!(
        reactivated.contains("published hook entry point"),
        "activation reports the copy it replaced: {reactivated}"
    );
    assert_eq!(
        fs::read(&published).expect("republished entry point reads"),
        fs::read(source).expect("entry point source reads"),
        "a stale copy is replaced by the bytes activation was handed"
    );
}

/// Byte-identical is not the same as usable, and Git needs the second one.
///
/// Activation's current-check compares bytes, so a copy left unexecutable by an
/// interrupted publish, a restrictive umask, or a restore from an archive reads
/// as current and activation would walk past it. Git then fails *every*
/// reference transaction in the repository, including the ones the guard exists
/// to allow. Clearing the bit on the copy the previous assertion just
/// republished isolates the mode: those bytes are already the ones activation
/// holds, so nothing but the mode is left for a passing run to be crediting.
fn assert_unusable_entry_point_is_repaired(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::set_permissions(&published, fs::Permissions::from_mode(0o644))
        .expect("clear the published entry point's executable bit");
    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("re-run activation over an unexecutable entry point");
    assert_eq!(
        mode(&published),
        0o755,
        "activation restores an entry point Git can exec"
    );

    // Creation is the one transaction the guard admits without looking at
    // history, which makes it the probe whose failure can only mean the hook
    // never ran: an unexecutable target fails the exec, the hook script exits
    // nonzero, and Git aborts an update that abandons nothing at all. The probe
    // reference is left in place afterwards on purpose -- deleting it would
    // point the guard at upstream-merged commits and be refused, which is the
    // guard doing its job rather than a cleanup worth arranging around.
    let probe = "refs/heads/entry-point-repair-probe";
    let head = rev_parse(scratch, "refs/heads/main");
    let created = Command::new("git")
        .args(["update-ref", probe, &head])
        .current_dir(scratch.path())
        .output()
        .expect("create the entry point probe reference");
    assert_success(
        &created,
        "create a reference through the repaired entry point",
    );
    assert_eq!(
        rev_parse(scratch, probe),
        head,
        "the admitted transaction was committed rather than aborted"
    );
}

/// Activation is the only writer of the guard stamp, so the audit is what makes
/// a stale publication visible to anyone who did not just run activation.
///
/// Build A is the real xtask executable activation publishes. Build B changes
/// `PROTECTED_COMMIT_LIMIT`, a runtime bound that changes the guard's refusal
/// account, and reaches the same production audit with the digest that source
/// would compile. B must name and refuse A. The explicit source-to-compiled
/// digest equality is what makes severing `include_str!` fail rather than
/// letting a synthetic sidecar mutation stand in for a build transition.
///
/// An absent stamp is refused separately. Re-activating at the end restores the
/// stamp for the assertions that follow and proves the already-current branch
/// of activation stamps as well, which is the branch a second lane running
/// activation over the first lane's copy takes.
fn assert_guard_audit_refuses_a_prior_build(scratch: &Scratch, source: &Path) {
    let stamp = published_guard_stamp(scratch.path());
    let published = fs::read_to_string(&stamp).expect("activation stamps the guard it published");
    let published = published.trim().to_owned();
    let guard_a_source = include_str!("../src/reference_transaction.rs");
    let behavior_a = "const PROTECTED_COMMIT_LIMIT: usize = 5;";
    let behavior_b = "const PROTECTED_COMMIT_LIMIT: usize = 6;";
    assert_eq!(
        guard_a_source.matches(behavior_a).count(),
        1,
        "the fixture changes one runtime guard bound rather than arbitrary prose"
    );
    let guard_b_source = guard_a_source.replacen(behavior_a, behavior_b, 1);
    let guard_a = xtask::compiled_reference_transaction_guard_digest();
    assert_eq!(
        guard_a,
        xtask::reference_transaction_guard_digest(guard_a_source.as_bytes()),
        "the published build identity is bound to the guard source it carries"
    );
    assert_eq!(published, format!("{guard_a:016x}"));
    let guard_b = xtask::reference_transaction_guard_digest(guard_b_source.as_bytes());
    assert_ne!(
        guard_b, guard_a,
        "changing the number of protected commits the guard reports moves the next build"
    );

    let mut current = Vec::new();
    xtask::audit_entry_point(scratch.path(), &mut current)
        .expect("a freshly published guard is this build's");
    let current = String::from_utf8(current).unwrap();
    assert!(
        current.contains(&published),
        "the audit names the guard it matched: {current}"
    );

    let stale = xtask::audit_entry_point_for_guard(scratch.path(), guard_b, &mut Vec::new())
        .expect_err("build B refuses the guard published by build A")
        .to_string();
    assert!(stale.contains(&published), "{stale}");
    assert!(stale.contains(&format!("{guard_b:016x}")), "{stale}");
    assert!(stale.contains("cargo xtask activate-hooks"), "{stale}");

    fs::remove_file(&stamp).expect("remove the published guard stamp");
    let unstamped = xtask::audit_entry_point(scratch.path(), &mut Vec::new())
        .expect_err("an entry point published before stamping existed is refused")
        .to_string();
    assert!(unstamped.contains("no stamp"), "{unstamped}");

    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("re-run activation over an already-current entry point");
    assert_eq!(
        fs::read_to_string(&stamp)
            .expect("the already-current branch stamps too")
            .trim(),
        published
    );
    xtask::audit_entry_point(scratch.path(), &mut Vec::new())
        .expect("the restored stamp is this build's");
}

/// An absent entry point aborts ref updates rather than degrading, which is
/// what makes activation a precondition of the install rather than a polish
/// step, and is now written in `README.md` where operators read it.
///
/// The claim is worth pinning because the alternative is plausible enough to
/// have been assumed: a hook that could not find its program might exit zero
/// and let the update through, which is how most optional tooling behaves and
/// is exactly the failure the guard must not have. Restoring afterwards also
/// records that absence self-heals -- the read fails and the copy is rewritten
/// -- so only the wrong-mode case above needed code to repair it.
fn assert_an_absent_entry_point_aborts_ref_updates(scratch: &Scratch, source: &Path) {
    let published = published_entry_point(scratch.path());
    fs::remove_file(&published).expect("remove the published entry point");
    let probe = "refs/heads/entry-point-absence-probe";
    let aborted = Command::new("git")
        .args(["update-ref", probe, &rev_parse(scratch, "refs/heads/main")])
        .current_dir(scratch.path())
        .output()
        .expect("attempt a reference update with no entry point published");
    assert_ne!(
        aborted.status.code(),
        Some(0),
        "an absent entry point aborts rather than admitting: {}",
        streams(&aborted)
    );
    assert!(
        String::from_utf8_lossy(&aborted.stderr).contains("aborted by hook"),
        "Git names the hook as the abort's cause: {}",
        streams(&aborted)
    );
    let listed = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", probe])
        .current_dir(scratch.path())
        .output()
        .expect("look for the reference the aborted update would have created");
    assert!(
        !listed.status.success(),
        "the aborted update left no reference behind: {}",
        streams(&listed)
    );

    xtask::activate_hooks(scratch.path(), source, &mut Vec::new())
        .expect("activation republishes an absent entry point");
    assert_eq!(mode(&published), 0o755);
}

fn published_guard_stamp(root: &Path) -> PathBuf {
    let repository = Repository::discover(root).expect("discover hook repository");
    repository
        .common_dir()
        .join(".spice-install/bin/xtask.guard")
}

/// The published copy is what the hook names, so the guarantee is read off that
/// file and not off the binary Cargo built.
///
/// Identity is asserted before the interface. A published file that merely
/// exists would let an interface check pass against some older publication, so
/// proving the bytes are the ones activation was handed is what makes the
/// answer below an answer from this build. Then the guard is driven through the
/// published path with a transaction it must refuse: an admitted one would exit
/// zero for a program that never looked at its input, while the refusal names
/// the guard's own sentence and so cannot be produced by anything else.
fn assert_entry_point_answers_the_guard(scratch: &Scratch, source: &Path, prepared: &str) {
    let published = published_entry_point(scratch.path());
    assert_eq!(
        mode(&published),
        0o755,
        "published entry point is executable"
    );
    assert_eq!(
        fs::read(&published).expect("published entry point reads"),
        fs::read(source).expect("entry point source reads"),
        "published entry point carries the bytes activation was handed"
    );

    let refusal = run_prepared(&published, scratch.path(), prepared);
    assert_ne!(
        refusal.status.code(),
        Some(0),
        "published entry point refuses a protected rewind: {}",
        streams(&refusal)
    );
    assert!(
        String::from_utf8_lossy(&refusal.stderr).contains("refused to abandon upstream-merged"),
        "published entry point answers the reference-transaction interface: {}",
        streams(&refusal)
    );
}

fn run_prepared(program: &Path, root: &Path, prepared: &str) -> Output {
    let mut child = Command::new(program)
        .args(["reference-transaction", "prepared"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run published entry point");
    child
        .stdin
        .take()
        .expect("published entry point accepts the prepared transaction")
        .write_all(prepared.as_bytes())
        .expect("write the prepared transaction");
    child.wait_with_output().expect("collect published output")
}

fn git_at(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run Git fixture query");
    assert_success(&output, "Git fixture query");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git(scratch: &Scratch, arguments: &[&str]) -> Output {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(scratch.path())
        .output()
        .expect("run Git fixture command");
    assert_success(&output, "Git fixture command");
    output
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: {}",
        streams(output)
    );
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn image(path: &Path) -> (Vec<u8>, u32) {
    (fs::read(path).unwrap(), mode(path))
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-native-hooks-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create temporary repository");
        let initialized = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(&path)
            .output()
            .expect("initialize temporary Git repository");
        assert_success(&initialized, "initialize temporary Git repository");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    fn executable(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        fs::create_dir_all(path.parent().unwrap()).expect("create executable parent");
        fs::write(&path, bytes).expect("write executable fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make fixture executable");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove temporary repository");
    }
}
