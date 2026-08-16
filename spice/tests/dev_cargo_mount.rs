//! Python's repository-mounted Cargo command enters the installed Rust runtime.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const PYTHON_CONFIGURATION: &str = include_str!("../../spice.toml");

const FAKE_RUST_SPICE: &[u8] = br#"#!/bin/sh
printf 'argv:'
for argument in "$@"; do
    printf '<%s>' "$argument"
done
printf '\n'
printf 'installed stderr\n' >&2
exit 37
"#;

#[test]
fn linked_worktree_mount_preserves_arguments_streams_and_status() {
    let fixture = Fixture::new("passthrough");
    fixture.write_installed(FAKE_RUST_SPICE, 0o755);

    let output = fixture
        .command()
        .args(["--", "--help", "space value"])
        .output()
        .expect("mounted launcher runs");

    assert_eq!(output.status.code(), Some(37));
    assert_eq!(
        output.stdout,
        b"argv:<dev><cargo><--><--help><space value>\n"
    );
    assert_eq!(output.stderr, b"installed stderr\n");
}

#[test]
fn mount_refuses_every_invalid_installed_identity() {
    let fixture = Fixture::new("refusals");

    let mut repository_absent = mounted_command(&fixture.scratch);
    assert_refusal(
        repository_absent.output().expect("repository refusal runs"),
        "repository-unresolved",
    );

    assert_refusal(
        fixture.command().output().expect("missing refusal runs"),
        "rust-command-unavailable",
    );

    fixture.write_installed(FAKE_RUST_SPICE, 0o644);
    assert_refusal(
        fixture.command().output().expect("invalid refusal runs"),
        "rust-command-unavailable",
    );

    fs::remove_file(fixture.installed()).expect("remove invalid installed command");
    let redirected = fixture.common.join("redirected-spice");
    write_executable(&redirected, FAKE_RUST_SPICE, 0o755);
    symlink(&redirected, fixture.installed()).expect("create redirected installed command");
    assert_refusal(
        fixture.command().output().expect("redirect refusal runs"),
        "rust-command-redirected",
    );
}

fn assert_refusal(output: Output, reason: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "unexpected output: {output:?}"
    );
    assert!(output.stdout.is_empty(), "refusal wrote stdout: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!("spice: coexistence::{reason}:")),
        "refusal omitted {reason}: {output:?}",
    );
}

struct Fixture {
    scratch: PathBuf,
    worktree: PathBuf,
    common: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let scratch = std::env::temp_dir().join(format!(
            "spice-dev-cargo-mount-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let repository = scratch.join("repository");
        let worktree = scratch.join("linked");
        fs::create_dir_all(&repository).expect("create mount fixture repository");
        git(&repository, ["init", "-q"]);
        git(
            &repository,
            ["config", "user.email", "mount@example.invalid"],
        );
        git(&repository, ["config", "user.name", "Mount Fixture"]);
        fs::write(repository.join("anchor"), b"anchor\n").expect("write fixture anchor");
        git(&repository, ["add", "anchor"]);
        git(&repository, ["commit", "-qm", "anchor"]);
        git(
            &repository,
            [
                "worktree",
                "add",
                "-q",
                "-b",
                "mounted-command",
                worktree.to_str().expect("fixture worktree path is UTF-8"),
            ],
        );
        let common = fs::canonicalize(repository.join(".git"))
            .expect("canonicalize fixture common Git directory");
        Self {
            scratch,
            worktree: fs::canonicalize(worktree).expect("canonicalize linked worktree"),
            common,
        }
    }

    fn installed(&self) -> PathBuf {
        self.common.join(".spice-install/bin/spice")
    }

    fn write_installed(&self, bytes: &[u8], mode: u32) {
        let installed = self.installed();
        fs::create_dir_all(installed.parent().expect("installed command has a parent"))
            .expect("create installed command directory");
        write_executable(&installed, bytes, mode);
    }

    fn command(&self) -> Command {
        mounted_command(&self.worktree)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.scratch)
            && !std::thread::panicking()
        {
            panic!("remove {}: {error}", self.scratch.display());
        }
    }
}

fn mounted_command(directory: &Path) -> Command {
    let parsed: toml::Table = PYTHON_CONFIGURATION
        .parse()
        .expect("Python tracked configuration parses as TOML");
    let argv = parsed["commands"]["dev.cargo"]
        .as_array()
        .expect("dev.cargo mount is argv")
        .iter()
        .map(|value| value.as_str().expect("mount argv item is text"))
        .collect::<Vec<_>>();
    let mut command = Command::new(argv[0]);
    command.args(&argv[1..]).current_dir(directory);
    command
}

fn write_executable(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write executable fixture");
    let mut permissions = fs::metadata(path)
        .expect("stat executable fixture")
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).expect("set executable fixture mode");
}

fn git<'a>(root: &Path, arguments: impl IntoIterator<Item = &'a str>) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git runs");
    assert!(
        output.status.success(),
        "fixture Git failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
