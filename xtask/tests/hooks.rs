#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{
    InstallPreview, InstallReceipt, InstallStatus, apply, installed_git_hooks, plan,
};
use spice::repository::Repository;

#[path = "hooks/activation_assertions.rs"]
mod activation_assertions;

use activation_assertions::assert_reference_transaction_hook_targets_installed_spice;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const ACTIVE_HOOKS: [&str; 3] = ["pre-commit", "commit-msg", "reference-transaction"];
const GIT_DIRECTORY: &str = concat!("GIT_", "DIR");
const GIT_WORKING_TREE: &str = concat!("GIT_", "WORK_TREE");
const GIT_INDEX: &str = concat!("GIT_", "INDEX_FILE");
const VALID_ORIGIN: &str =
    "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=HOOKS-1kJQnmG8";
type FileImage = (Vec<u8>, u32);

#[test]
fn install_exercises_nontransaction_hooks_before_explicit_activation() {
    let scratch = Scratch::new();
    repository_history(&scratch);
    let installation = install_spice_hooks(&scratch);
    let cargo_log = scratch.join("cargo.log");
    install_cargo_alias(&scratch, &cargo_log);

    exercise_pre_commit(&scratch, &cargo_log);
    exercise_commit_message(&scratch);
    assert_prior_hooks_unchanged(&scratch, &installation);
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
    let installed_hooks = ACTIVE_HOOKS.map(|hook| image(&common_hooks(&scratch).join(hook)));
    install_refusal_delegate(&scratch, &predecessor);
    let installed_entry = image(&installed_spice(&scratch));
    let mut output = Vec::new();

    let error = xtask::activate_hooks(scratch.path(), &mut output)
        .expect_err("foreign predecessor hooks must refuse activation");

    assert!(
        matches!(
            error,
            xtask::HookActivationError::InstalledExit { code: Some(2), .. }
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("install::foreign-hooks"),
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
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&common_hooks(&scratch).join(hook))),
        installed_hooks
    );
    assert_eq!(image(&installed_spice(&scratch)), installed_entry);
    assert_eq!(configured_hooks(&scratch), ".githooks");
    assert_eq!(configured_hooks_at(&predecessor), ".spice/hooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&installation.operator.join(hook))),
        installation.operator_hooks
    );
}

#[test]
fn activation_delegates_preview_and_exact_digest_apply_from_the_repository_root() {
    let scratch = Scratch::new();
    repository_history(&scratch);
    let installation = install_spice_hooks(&scratch);
    let (digest, log) = install_success_delegate(&scratch, &installation);
    let nested = scratch.join("nested/caller");
    fs::create_dir_all(&nested).expect("create nested activation caller");
    let mut output = Vec::new();

    xtask::activate_hooks(&nested, &mut output).expect("installed delegation completes");

    let canonical_root = fs::canonicalize(scratch.path()).expect("canonicalize fixture root");
    assert_eq!(
        fs::read_to_string(log).expect("read installed delegation log"),
        format!(
            "{}|install\n{}|install --apply={digest}\n",
            canonical_root.display(),
            canonical_root.display(),
        )
    );
    assert_eq!(
        String::from_utf8(output).expect("activation output is UTF-8"),
        format!("activated Spice hooks through installed plan {digest}\n")
    );
}

fn install_success_delegate(
    scratch: &Scratch,
    installation: &PriorHookEvidence,
) -> (String, PathBuf) {
    let preview = InstallPreview::from_plan(installation.receipt.plan().clone())
        .expect("installed fixture plan previews");
    assert_eq!(preview.digest(), installation.receipt.digest());
    let preview = String::from_utf8(wire::encode(&preview).expect("encode delegated preview"))
        .expect("delegated preview is UTF-8");
    let receipt =
        String::from_utf8(wire::encode(&installation.receipt).expect("encode delegated receipt"))
            .expect("delegated receipt is UTF-8");
    assert!(!preview.contains('\''));
    assert!(!receipt.contains('\''));
    let digest = installation.receipt.digest().to_string();
    let log = scratch.join("installed-delegation.log");
    assert!(!log.to_string_lossy().contains('\''));
    write_installed_spice(
        scratch,
        format!(
            concat!(
                "#!/bin/sh -eu\n",
                "printf '%s|%s\\n' \"$PWD\" \"$*\" >> '{}'\n",
                "case \"$*\" in\n",
                "  install) printf '%s\\n' '{}' ;;\n",
                "  'install --apply={}') printf '%s\\n' '{}' ;;\n",
                "  *) exit 64 ;;\n",
                "esac\n",
            ),
            log.display(),
            preview,
            digest,
            receipt,
        )
        .as_bytes(),
    );
    (digest, log)
}

fn install_refusal_delegate(scratch: &Scratch, predecessor: &Path) {
    assert!(!predecessor.to_string_lossy().contains('\''));
    write_installed_spice(
        scratch,
        format!(
            "#!/bin/sh\nprintf '%s\\n' 'spice: install::foreign-hooks; refusing foreign core.hooksPath [\".spice/hooks\"] in {}' >&2\nexit 2\n",
            predecessor.display(),
        )
        .as_bytes(),
    );
}

fn write_installed_spice(scratch: &Scratch, bytes: &[u8]) {
    let path = installed_spice(scratch);
    fs::write(&path, bytes).expect("write installed Spice delegate");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("make installed Spice delegate executable");
}

fn installed_spice(scratch: &Scratch) -> PathBuf {
    Repository::discover(scratch.path())
        .expect("discover installed Spice path")
        .common_dir()
        .join(".spice-install/bin/spice")
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
    let operator_before = ACTIVE_HOOKS.map(|hook| image(&operator.join(hook)));
    let tracked_before = ACTIVE_HOOKS.map(|hook| image(&tracked.join(hook)));
    let repository = Repository::discover(scratch.path()).expect("discover temporary repository");
    let spice = fixture_spice(scratch);
    let planned = plan(&repository, Some(&spice)).expect("plan Spice hooks");
    let receipt = apply(
        &repository,
        Some(&spice),
        &planned.digest().expect("digest plan"),
    )
    .expect("install Spice hooks");
    assert_eq!(receipt.status(), InstallStatus::Complete);
    git(
        scratch,
        &["config", "--local", "core.hooksPath", ".githooks"],
    );
    let config_before = image(&scratch.join(".git/config"));

    let installed_hooks = installed_git_hooks(&repository);
    let pre_commit = installed_hooks.join("pre-commit");
    let commit_message = installed_hooks.join("commit-msg");
    let reference_transaction = installed_hooks.join("reference-transaction");
    assert_eq!(mode(&pre_commit), 0o755);
    assert_eq!(mode(&commit_message), 0o755);
    assert_eq!(mode(&reference_transaction), 0o755);
    assert_reference_transaction_hook_targets_installed_spice(scratch);
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
        receipt,
    }
}

fn tracked_current_hook(hook: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join(".githooks").join(hook);
    let bytes = fs::read(&path).expect("read tracked native hook wrapper");
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("spice dev"));
    if hook == "pre-commit" {
        assert_eq!(
            text.matches("cargo --quiet xtask verify-action selection")
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
        assert!(text.contains("/.spice-install/bin/spice\" gate reference-transaction"));
    }
    bytes
}

fn fixture_spice(scratch: &Scratch) -> PathBuf {
    let dispatcher = scratch.join("bin/xtask-dispatcher");
    assert!(!dispatcher.to_string_lossy().contains('\''));
    scratch.executable(
        "fixture-spice",
        format!(
            "#!/bin/sh -eu\n[ \"$1\" = gate ]\nhook=$2\nshift 2\ncase \"$hook\" in\n  pre-commit)\n    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE\n    exec '{}' verify-action selection \"$@\"\n    ;;\n  commit-msg) exec '{}' commit-msg \"$@\" ;;\n  reference-transaction) exit 0 ;;\n  *) exit 64 ;;\nesac\n",
            dispatcher.display(),
            dispatcher.display(),
        )
        .as_bytes(),
    )
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
        "verify-action selection\n"
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

struct PriorHookEvidence {
    operator: PathBuf,
    operator_hooks: [FileImage; 3],
    tracked: PathBuf,
    tracked_hooks: [FileImage; 3],
    config: FileImage,
    receipt: InstallReceipt,
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
                "[ \"$2\" = selection ]; then\n",
                "    exit 0\n",
                "fi\n",
                "if [ \"$#\" -eq 2 ] && [ \"$1\" = commit-msg ]; then\n",
                "    exec '{}' commit-msg \"$2\"\n",
                "fi\n",
                // No reference-transaction arm on purpose. The hook execs the
                // installed Spice binary directly, so routing that subcommand
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
