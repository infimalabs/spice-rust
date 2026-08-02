#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{InstallStatus, apply, plan};
use spice::repository::Repository;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const ACTIVE_HOOKS: [&str; 3] = ["pre-commit", "commit-msg", "reference-transaction"];
const NATIVE_HOOKS: &str = ".spice/rust/git-hooks";
const VALID_ORIGIN: &str =
    "Spice-Origin: v1 repo=018f82d4-6f59-7d06-92e3-bce9131163c7 task=HOOKS-1kJQnmG8";
type FileImage = (Vec<u8>, u32);

#[test]
fn install_exercises_native_hooks_before_an_explicit_byte_preserving_cutover() {
    let scratch = Scratch::new();
    let (base, protected, local) = repository_history(&scratch);
    let coexistence = install_native_hooks(&scratch);
    let cargo_log = scratch.join("cargo.log");
    install_cargo_alias(&scratch, &cargo_log);

    exercise_pre_commit(&scratch, &cargo_log);
    exercise_commit_message(&scratch);
    assert_coexistence_unchanged(&scratch, &coexistence);
    exercise_reference_guard(&scratch, &base, &protected, &local);
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&coexistence.active.join(hook))),
        coexistence.hooks
    );
}

fn install_native_hooks(scratch: &Scratch) -> CoexistenceEvidence {
    let active = scratch.join(".spice/hooks");
    fs::create_dir_all(&active).expect("create active Python hooks");
    for hook in ACTIVE_HOOKS {
        scratch.executable(
            &format!(".spice/hooks/{hook}"),
            format!("#!/bin/sh\nexec spice dev {hook} \"$@\"\n").as_bytes(),
        );
    }
    git(
        scratch,
        &["config", "--local", "core.hooksPath", ".spice/hooks"],
    );
    let active_before = ACTIVE_HOOKS.map(|hook| image(&active.join(hook)));
    let config_before = image(&scratch.join(".git/config"));
    let repository = Repository::discover(scratch.path()).expect("discover temporary repository");
    let planned = plan(&repository, None).expect("plan native hooks");
    let receipt = apply(&repository, None, &planned.digest().expect("digest plan"))
        .expect("install native hooks");
    assert_eq!(receipt.status(), InstallStatus::Complete);

    let pre_commit = scratch.join(format!("{NATIVE_HOOKS}/pre-commit"));
    let commit_message = scratch.join(format!("{NATIVE_HOOKS}/commit-msg"));
    let reference_transaction = scratch.join(format!("{NATIVE_HOOKS}/reference-transaction"));
    assert_eq!(mode(&pre_commit), 0o755);
    assert_eq!(mode(&commit_message), 0o755);
    assert_eq!(mode(&reference_transaction), 0o755);
    assert_eq!(configured_hooks(scratch), ".spice/hooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&active.join(hook))),
        active_before
    );
    assert_eq!(image(&scratch.join(".git/config")), config_before);
    CoexistenceEvidence {
        active,
        hooks: active_before,
        config: config_before,
    }
}

fn exercise_pre_commit(scratch: &Scratch, cargo_log: &Path) {
    let pre_commit = scratch.join(format!("{NATIVE_HOOKS}/pre-commit"));
    let pre_commit_output = invoke(&pre_commit, scratch.path(), &[]);
    assert_success(&pre_commit_output, "pre-commit");
    assert_eq!(fs::read_to_string(cargo_log).unwrap(), "verify\n");
}

fn exercise_commit_message(scratch: &Scratch) {
    let commit_message = scratch.join(format!("{NATIVE_HOOKS}/commit-msg"));
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
}

fn assert_coexistence_unchanged(scratch: &Scratch, evidence: &CoexistenceEvidence) {
    assert_eq!(configured_hooks(scratch), ".spice/hooks");
    assert_eq!(
        ACTIVE_HOOKS.map(|hook| image(&evidence.active.join(hook))),
        evidence.hooks
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

    let cutover = scratch.executable(
        "scripts/install-hooks.sh",
        include_bytes!("../../scripts/install-hooks.sh"),
    );
    let activated = Command::new(cutover)
        .current_dir(scratch.path())
        .output()
        .expect("run explicit hook cutover");
    assert_success(&activated, "hook cutover");
    assert_eq!(configured_hooks(scratch), NATIVE_HOOKS);
    assert!(
        String::from_utf8(activated.stdout)
            .unwrap()
            .contains("activated Rust hooks")
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

struct CoexistenceEvidence {
    active: PathBuf,
    hooks: [FileImage; 3],
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
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "if [ \"$#\" -eq 1 ] && [ \"$1\" = verify ]; then\n",
                "    exit 0\n",
                "fi\n",
                "if [ \"$#\" -eq 2 ] && [ \"$1\" = commit-msg ]; then\n",
                "    exec '{}' commit-msg \"$2\"\n",
                "fi\n",
                "if [ \"$#\" -eq 2 ] && [ \"$1\" = reference-transaction ]; then\n",
                "    exec '{}' reference-transaction \"$2\"\n",
                "fi\n",
                "printf 'unexpected xtask invocation: %s\\n' \"$*\" >&2\n",
                "exit 64\n",
            ),
            log.display(),
            xtask.display(),
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
    String::from_utf8(
        git(
            scratch,
            &["config", "--worktree", "--get", "core.hooksPath"],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
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
