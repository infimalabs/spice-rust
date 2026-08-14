#![cfg(unix)]

//! End-to-end proof that one installed appliance owns all three Git hook rims
//! without a source checkout, build-system executable, or resident process.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{InstallStatus, apply, installed_binary, installed_git_hooks, plan, unapply};
use spice::repository::{Repository, StatePaths};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn generic_repository_runs_the_complete_installed_hook_appliance() {
    let fixture = Fixture::new();
    let repository = Repository::discover(fixture.path()).expect("discover generic repository");
    let state = repository
        .admit_state()
        .expect("admit generic repository state paths");
    assert_generic_inputs(&fixture);
    let (installed, hooks) = install_appliance(&repository);
    assert_installed_launchers(&fixture, &installed, &hooks);
    commit_rewritten_message(&fixture, &state);
    let protected = refuse_then_bypass_commit(&fixture, &state);
    assert_reference_protection(&fixture, &protected);
    assert_daemonless_build_free_runtime(&fixture, &state);
    reverse_appliance(&fixture, &repository, &installed);
}

fn assert_generic_inputs(fixture: &Fixture) {
    for absent in ["Cargo.toml", "spice-rust.toml", "spice.toml"] {
        assert!(
            !fixture.join(absent).exists(),
            "generic fixture unexpectedly contains {absent}",
        );
    }
}

fn install_appliance(repository: &Repository) -> (PathBuf, PathBuf) {
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let preview = plan(repository, Some(source)).expect("preview complete appliance install");
    let receipt = apply(
        repository,
        Some(source),
        &preview.digest().expect("digest complete appliance plan"),
    )
    .expect("install complete appliance");
    assert_eq!(receipt.status(), InstallStatus::Complete);

    let installed = installed_binary(repository);
    let hooks = installed_git_hooks(repository);
    (installed, hooks)
}

fn assert_installed_launchers(fixture: &Fixture, installed: &Path, hooks: &Path) {
    assert!(installed.is_file());
    assert_eq!(
        fixture.git_text(&["config", "--get", "core.hooksPath"]),
        hooks.to_string_lossy(),
    );
    for (name, leaf) in [
        ("pre-commit", "pre-commit"),
        ("commit-msg", "commit-msg"),
        ("reference-transaction", "reference-transaction"),
    ] {
        let launcher = fs::read_to_string(hooks.join(name)).expect("read installed hook launcher");
        assert_eq!(launcher.lines().count(), 2);
        assert!(launcher.contains(&format!(" gate {leaf} \"$@\"")));
        assert!(!launcher.contains("cargo") && !launcher.contains("xtask"));
    }
}

fn commit_rewritten_message(fixture: &Fixture, state: &StatePaths) {
    fixture.write(
        "README.md",
        "# Generic repository\n\nChanged documentation.\n",
    );
    fixture.git(&["add", "README.md"]);
    let long_message = fixture.join("message with spaces and 'apostrophe'");
    fs::write(
        &long_message,
        format!(
            "Validate and fold generic commits\n\n{}\n",
            "The installed hook must wrap this deliberately long body without entering a build system. "
                .repeat(4),
        ),
    )
    .expect("write long commit message");
    let committed = fixture
        .git_command(&["commit", "-q", "-F"])
        .arg(&long_message)
        .output()
        .expect("commit through installed pre-commit and commit-msg hooks");
    assert_success(&committed, "generic commit through installed hooks");
    assert!(
        String::from_utf8_lossy(&committed.stderr)
            .contains("auto-folded commit message body lines"),
        "commit-msg hook did not report its rewrite: {}",
        streams(&committed),
    );
    let committed_message = fixture.git_text(&["show", "-s", "--format=%B", "HEAD"]);
    assert!(
        committed_message
            .lines()
            .all(|line| line.chars().count() <= 100)
    );
    assert!(
        state.gate_state().is_file(),
        "packaged pre-commit did not publish its accepted staged identity",
    );
}

fn refuse_then_bypass_commit(fixture: &Fixture, state: &StatePaths) -> String {
    fixture.write(
        "README.md",
        "# Generic repository\n\nChanged documentation again.\n",
    );
    fixture.git(&["add", "README.md"]);
    let before_refusal = fixture.git_text(&["rev-parse", "HEAD"]);
    let refused = fixture
        .git_command(&["commit", "-q", "-m", "WIP"])
        .output()
        .expect("run refused commit through installed hooks");
    assert!(
        !refused.status.success(),
        "WIP message unexpectedly committed"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("placeholder"),
        "commit-message refusal lost its evidence: {}",
        streams(&refused),
    );
    assert_eq!(fixture.git_text(&["rev-parse", "HEAD"]), before_refusal);
    assert_eq!(
        fixture.git_text(&["diff", "--cached", "--name-only"]),
        "README.md"
    );
    let gate_state_before_bypass =
        fs::read(state.gate_state()).expect("read gate state before explicit bypass");

    let bypassed = fixture
        .git_command(&["commit", "--no-verify", "-q", "-m", "WIP"])
        .output()
        .expect("commit with Git's explicit hook bypass");
    assert_success(&bypassed, "--no-verify commit");
    assert_eq!(
        fixture.git_text(&["show", "-s", "--format=%s", "HEAD"]),
        "WIP"
    );
    assert_eq!(
        fs::read(state.gate_state()).expect("read gate state after explicit bypass"),
        gate_state_before_bypass,
        "--no-verify unexpectedly entered the packaged pre-commit gate",
    );
    fixture.git_text(&["rev-parse", "HEAD"])
}

fn assert_reference_protection(fixture: &Fixture, protected: &str) {
    let base = fixture.git_text(&["rev-parse", "HEAD^"]);
    fixture.git(&["remote", "add", "origin", "."]);
    fixture.git(&["update-ref", "refs/remotes/origin/main", protected]);
    fixture.git(&["config", "branch.main.remote", "origin"]);
    fixture.git(&["config", "branch.main.merge", "refs/heads/main"]);
    let rewind = fixture
        .git_command(&["update-ref", "refs/heads/main", &base, protected])
        .output()
        .expect("rewind through installed reference-transaction hook");
    assert!(
        !rewind.status.success(),
        "protected history unexpectedly rewound"
    );
    assert!(
        String::from_utf8_lossy(&rewind.stderr)
            .contains("refused to abandon upstream-merged commits"),
        "reference refusal lost its evidence: {}",
        streams(&rewind),
    );
    assert_eq!(fixture.git_text(&["rev-parse", "HEAD"]), protected);
}

fn assert_daemonless_build_free_runtime(fixture: &Fixture, state: &StatePaths) {
    assert!(
        !fixture.tripwire().exists(),
        "generic installed hooks entered Cargo, xtask, or PATH-provided Spice: {}",
        fs::read_to_string(fixture.tripwire()).unwrap_or_default(),
    );
    assert!(
        !state.resident().exists(),
        "daemonless hooks created a resident possession file",
    );
}

fn reverse_appliance(fixture: &Fixture, repository: &Repository, installed: &Path) {
    let reversed = unapply(repository).expect("reverse complete appliance installation");
    assert_eq!(reversed.status(), InstallStatus::Uninstalled);
    assert!(!installed.exists());
    assert!(
        fixture
            .git_optional_text(&["config", "--get", "core.hooksPath"])
            .is_none(),
        "unapply did not restore the absent predecessor hook selection",
    );
}

struct Fixture {
    root: PathBuf,
    path: std::ffi::OsString,
}

impl Fixture {
    fn new() -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice hook appliance 'generic' {} {sequence}",
            std::process::id(),
        ));
        fs::create_dir(&root).expect("create special-path appliance fixture");
        let programs = root.join("tripwire programs");
        fs::create_dir(&programs).expect("create tripwire program directory");
        for name in ["cargo", "xtask", "spice"] {
            let program = programs.join(name);
            fs::write(
                &program,
                concat!(
                    "#!/bin/sh\n",
                    "printf '%s\\n' \"$0 $*\" >> \"$PWD/runtime tripwire invoked\"\n",
                    "exit 91\n",
                ),
            )
            .expect("write runtime tripwire");
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
                .expect("make runtime tripwire executable");
        }
        let path = std::env::join_paths([
            programs.clone(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .expect("compose runtime tripwire PATH");
        let fixture = Self { root, path };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Generic Hook Appliance"]);
        fixture.git(&["config", "user.email", "generic-hooks@example.invalid"]);
        fixture.write("README.md", "# Generic repository\n");
        fixture.git(&["add", "README.md"]);
        fixture.git(&["commit", "-q", "-m", "Create generic repository"]);
        fixture
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.join(path)
    }

    fn tripwire(&self) -> PathBuf {
        self.root.join("runtime tripwire invoked")
    }

    fn write(&self, path: &str, text: &str) {
        fs::write(self.join(path), text).expect("write generic repository source");
    }

    fn git(&self, arguments: &[&str]) {
        let output = self
            .git_command(arguments)
            .output()
            .expect("run fixture Git command");
        assert_success(&output, &format!("git {}", arguments.join(" ")));
    }

    fn git_text(&self, arguments: &[&str]) -> String {
        let output = self
            .git_command(arguments)
            .output()
            .expect("run fixture Git query");
        assert_success(&output, &format!("git {}", arguments.join(" ")));
        String::from_utf8(output.stdout)
            .expect("Git output is UTF-8")
            .trim()
            .to_owned()
    }

    fn git_optional_text(&self, arguments: &[&str]) -> Option<String> {
        let output = self
            .git_command(arguments)
            .output()
            .expect("run optional fixture Git query");
        if output.status.success() {
            return Some(
                String::from_utf8(output.stdout)
                    .expect("Git output is UTF-8")
                    .trim()
                    .to_owned(),
            );
        }
        assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
        assert!(output.stdout.is_empty(), "{}", streams(&output));
        assert!(output.stderr.is_empty(), "{}", streams(&output));
        None
    }

    fn git_command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new("/usr/bin/git");
        command
            .args(arguments)
            .current_dir(&self.root)
            .env("PATH", &self.path)
            .env_remove(wire::SpiceVariable::PreCommitPaths.as_str())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(steer::hook::INBOX);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove appliance fixture");
    }
}

fn assert_success(output: &Output, label: &str) {
    assert!(output.status.success(), "{label}: {}", streams(output));
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
