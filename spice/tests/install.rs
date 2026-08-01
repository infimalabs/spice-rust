#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{InstallPlan, InstallReceipt, InstallStatus, apply, plan, unapply};
use spice::repository::Repository;
use wire::{Outcome, ReplyPosition};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const MAX_COMPACT_RECORD_BYTES: u64 = 64 * 1024;

#[test]
fn installed_rust_binary_coexists_with_the_python_entrypoint() {
    let scratch = Scratch::new();
    let repository = Repository::discover(scratch.path()).unwrap();
    let python = scratch.executable(
        "bin/spice",
        b"#!/usr/bin/env python3\nprint('python-spice')\n",
    );
    let python_before = fs::read(&python).unwrap();
    let rust = Path::new(env!("CARGO_BIN_EXE_spice"));

    let planned = plan(&repository, Some(rust)).unwrap();
    let receipt = apply(&repository, Some(rust), &planned.digest().unwrap()).unwrap();
    let installed = scratch.join(".spice/rust/bin/spice-rust");

    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(run(&python, &[]), b"python-spice\n");
    let help = run(&installed, &["--help"]);
    assert!(
        String::from_utf8(help)
            .unwrap()
            .contains("Repository-local coordination appliance")
    );
    assert_eq!(fs::read(&python).unwrap(), python_before);
    assert_eq!(
        fs::metadata(&installed).unwrap().len(),
        fs::metadata(rust).unwrap().len()
    );
    assert!(
        fs::metadata(scratch.join(".git/.spice-rust/install.json"))
            .unwrap()
            .len()
            < MAX_COMPACT_RECORD_BYTES
    );

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed.exists());
    assert_eq!(run(&python, &[]), b"python-spice\n");
}

#[test]
fn cli_preview_apply_and_unapply_are_typed_idempotent_and_python_first() {
    let scratch = Scratch::new();
    let python = scratch.executable(
        "bin/spice",
        b"#!/usr/bin/env python3\nprint('python-spice')\n",
    );
    let hook = scratch.executable(
        ".spice/hooks/pre-commit",
        b"#!/bin/sh\nexec spice dev pre-commit \"$@\"\n",
    );
    let git_config = scratch.join(".git/config");
    fs::write(&git_config, b"[core]\n\thooksPath = .spice/hooks\n").unwrap();
    let python_before = image(&python);
    let hook_before = image(&hook);
    let config_before = image(&git_config);

    let invalid = invoke(
        scratch.path(),
        &["--json", "install", "--apply=not-a-digest"],
    );
    assert_eq!(invalid.status.code(), Some(2));
    let invalid: Outcome<InstallReceipt> = wire::decode(&invalid.stderr).unwrap();
    let Outcome::Refusal(refusal) = invalid else {
        panic!("invalid install digest must refuse");
    };
    assert_eq!(refusal.reason, "install::digest-refused");
    assert!(!scratch.join(".spice/rust").exists());

    let text_plan = invoke(scratch.path(), &["install"]);
    let json_plan = invoke(scratch.path(), &["--json", "install"]);
    assert!(text_plan.status.success());
    assert!(json_plan.status.success());
    let text_plan: InstallPlan = wire::decode(&text_plan.stdout).unwrap();
    let json_plan: Outcome<InstallPlan> = wire::decode(&json_plan.stdout).unwrap();
    let Outcome::Reply(json_plan) = json_plan else {
        panic!("JSON install preview must reply");
    };
    assert_eq!(json_plan.position, ReplyPosition::Direct {});
    assert_eq!(json_plan.body, text_plan);
    assert!(!scratch.join(".spice/rust").exists());

    let apply_argument = format!("--apply={}", text_plan.digest().unwrap());
    let text_apply = invoke(scratch.path(), &["install", &apply_argument]);
    let json_apply = invoke(scratch.path(), &["--json", "install", &apply_argument]);
    let text_apply: InstallReceipt = successful_body(text_apply);
    let json_apply: InstallReceipt = successful_reply(json_apply);
    assert_eq!(text_apply, json_apply);
    assert_eq!(text_apply.status(), InstallStatus::Complete);

    let text_unapply: InstallReceipt =
        successful_body(invoke(scratch.path(), &["install", "--unapply"]));
    let json_unapply: InstallReceipt =
        successful_reply(invoke(scratch.path(), &["--json", "install", "--unapply"]));
    assert_eq!(text_unapply, json_unapply);
    assert_eq!(text_unapply.status(), InstallStatus::Uninstalled);

    assert_eq!(image(&python), python_before);
    assert_eq!(image(&hook), hook_before);
    assert_eq!(image(&git_config), config_before);
    assert_eq!(run(&python, &[]), b"python-spice\n");
}

fn invoke(repository: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(arguments)
        .current_dir(repository)
        .env_remove(steer::hook::INBOX)
        .output()
        .unwrap()
}

fn successful_body<T: serde::de::DeserializeOwned>(output: Output) -> T {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    wire::decode(&output.stdout).unwrap()
}

fn successful_reply<T: serde::de::DeserializeOwned>(output: Output) -> T {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let outcome: Outcome<T> = wire::decode(&output.stdout).unwrap();
    let Outcome::Reply(reply) = outcome else {
        panic!("JSON install command must reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply.body
}

fn image(path: &Path) -> (Vec<u8>, u32) {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().permissions().mode(),
    )
}

fn run(path: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(path).args(arguments).output().unwrap();
    assert!(output.status.success(), "{} failed", path.display());
    assert!(output.stderr.is_empty(), "{} wrote stderr", path.display());
    output.stdout
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        for _ in 0..100 {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-executable-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    fs::create_dir_all(root.join(".git/info")).unwrap();
                    fs::write(root.join(".git/info/exclude"), b"").unwrap();
                    return Self(root);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create {}: {error}", root.display()),
            }
        }
        panic!("no unique install integration fixture path was available");
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    fn executable(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && !std::thread::panicking()
        {
            panic!("remove {}: {error}", self.0.display());
        }
    }
}
