use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const GIT: &str = "/usr/bin/git";
const PARENT_GIT_ENVIRONMENT: [&str; 7] = [
    "GIT_DIR",
    "GIT_CONFIG",
    "GIT_INDEX_FILE",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
];

pub(super) struct ReportFixture {
    pub(super) root: PathBuf,
}

impl ReportFixture {
    pub(super) fn new() -> Self {
        let root = unique_root("spice-gate-report");
        fs::create_dir(&root).expect("create report fixture root");
        let fixture = Self { root };
        fs::create_dir(fixture.root.join("src")).expect("create report fixture source directory");
        fs::write(
            fixture.root.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", crate::NARROWEST_FILE_CLASSES),
        )
        .expect("write report fixture policy");
        fs::write(
            fixture.root.join("baseline.toml"),
            concat!(
                "version = 1\n\n",
                "[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
            ),
        )
        .expect("write report fixture baseline");
        fs::write(fixture.root.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write report fixture source");
        for arguments in [
            ["init", "-q", "-b", "main"].as_slice(),
            ["config", "user.name", "Gate Report Fixture"].as_slice(),
            ["config", "user.email", "gate-report@example.invalid"].as_slice(),
            ["add", "--all"].as_slice(),
            ["commit", "-q", "-m", "report fixture"].as_slice(),
        ] {
            run_git(&fixture.root, arguments);
        }
        fixture
    }

    pub(super) fn assert_parent_environment_isolated() {
        let mut command = Command::new(GIT);
        for variable in PARENT_GIT_ENVIRONMENT {
            command.env(variable, "parent-repository");
        }
        isolate_git(&mut command, Path::new("/fixture"));
        for variable in PARENT_GIT_ENVIRONMENT {
            assert!(
                command
                    .get_envs()
                    .all(|(name, _)| name != std::ffi::OsStr::new(variable)),
                "report fixture retained parent variable {variable}"
            );
        }
    }
}

impl Drop for ReportFixture {
    fn drop(&mut self) {
        remove_tree(&self.root);
    }
}

fn scoped_git(root: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(GIT);
    command.args(arguments).current_dir(root);
    isolate_git(&mut command, root);
    command
}

fn isolate_git(command: &mut Command, root: &Path) {
    command
        .env_clear()
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1");
}

fn run_git(root: &Path, arguments: &[&str]) {
    let output = scoped_git(root, arguments)
        .output()
        .unwrap_or_else(|error| panic!("launch git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn unique_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_tree(root: &Path) {
    if let Err(error) = fs::remove_dir_all(root) {
        eprintln!(
            "failed to remove report fixture tree {}: {error}",
            root.display()
        );
    }
}
