#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMAGE: &str = "sha256:stage-app-runner-fixture";
const LOCK: &str = ".spice-stage-app.lock";

#[test]
fn linked_worktree_mounts_its_common_repository_read_only_and_holds_the_lock() {
    let fixture = Fixture::new("mounts");

    let output = fixture.run();

    assert_success(&output, "linked-worktree staging runner");
    let arguments = fs::read_to_string(&fixture.docker_log).expect("read Docker argument log");
    let workspace_mount = format!(
        "type=bind,source={},target=/workspace",
        fixture.worktree.display()
    );
    let common_mount = format!(
        "type=bind,source={0},target={0},readonly",
        fixture.common.display()
    );
    for expected in [
        "run",
        "--rm",
        "--platform",
        "linux/amd64",
        "--network",
        "none",
        &workspace_mount,
        &common_mount,
        "type=volume,source=spice-stage-target,target=/cargo-target",
        "CARGO_HOME=/usr/local/cargo",
        "CARGO_TARGET_DIR=/cargo-target",
        IMAGE,
        "cargo",
        "xtask",
        "stage-app",
    ] {
        assert!(
            arguments.lines().any(|argument| argument == expected),
            "Docker invocation omitted {expected:?}:\n{arguments}",
        );
    }
    assert!(
        arguments.contains("lock-held\n"),
        "the Docker child did not observe the repository-common lock: {arguments}",
    );
    assert!(
        !fixture.common.join(LOCK).exists(),
        "the runner retained its lock after the Docker child exited",
    );
}

#[test]
fn a_contended_common_repository_lock_launches_no_docker_child() {
    let fixture = Fixture::new("contention");
    fs::create_dir(fixture.common.join(LOCK)).expect("create contended stage-app lock");

    let output = fixture.run();

    assert_eq!(
        output.status.code(),
        Some(75),
        "unexpected refusal: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stage-app is already running"),
        "contention refusal did not name the active runner: {output:?}",
    );
    assert!(
        !fixture.docker_log.exists(),
        "a contended runner launched Docker before refusing",
    );
}

struct Fixture {
    root: PathBuf,
    worktree: PathBuf,
    common: PathBuf,
    runner: PathBuf,
    docker_log: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-stage-app-{name}-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale stage-app fixture");
        }
        let primary = root.join("primary");
        let worktree_path = root.join("linked");
        fs::create_dir_all(&primary).expect("create primary repository");
        git(&primary, &["init", "-q"]);
        fs::write(primary.join("seed"), "seed\n").expect("write seed file");
        git(&primary, &["add", "seed"]);
        git(
            &primary,
            &[
                "-c",
                "user.name=Stage Fixture",
                "-c",
                "user.email=stage@example.invalid",
                "commit",
                "-qm",
                "seed",
            ],
        );
        git(
            &primary,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                worktree_path.to_str().expect("UTF-8 worktree path"),
            ],
        );
        let worktree = fs::canonicalize(worktree_path).expect("resolve linked worktree path");
        let common = PathBuf::from(git_output(
            &worktree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ));

        let proof = worktree.join("dist/proof-image");
        fs::create_dir_all(&proof).expect("create proof-image directory");
        let runner = proof.join("stage-app.sh");
        let source = fs::read_to_string(repository_runner()).expect("read staging runner");
        let body = source
            .strip_prefix("#!/bin/sh\n")
            .expect("staging runner has a shell header");
        let docker_log = root.join("docker-arguments");
        let instrumented = format!(
            "#!/bin/sh\n\
             docker() {{\n\
                 test -d '{lock}'\n\
                 printf 'lock-held\\n' > '{log}'\n\
                 printf '%s\\n' \"$@\" >> '{log}'\n\
             }}\n\
             {body}",
            lock = common.join(LOCK).display(),
            log = docker_log.display(),
        );
        fs::write(&runner, instrumented).expect("write instrumented staging runner");
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755))
            .expect("make staging runner executable");
        fs::write(proof.join("identity.txt"), format!("{IMAGE}\n"))
            .expect("write fixture image identity");

        Self {
            root,
            worktree,
            common,
            runner,
            docker_log,
        }
    }

    fn run(&self) -> Output {
        Command::new(&self.runner)
            .current_dir(self.root.as_path())
            .output()
            .expect("staging runner launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove stage-app fixture");
    }
}

fn repository_runner() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace parent")
        .join("dist/proof-image/stage-app.sh")
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert_success(&output, "fixture Git command");
}

fn git_output(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git query launches");
    assert_success(&output, "fixture Git query");
    String::from_utf8(output.stdout)
        .expect("fixture Git output is UTF-8")
        .trim()
        .to_owned()
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
