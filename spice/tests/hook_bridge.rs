#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::config::{Document, Inputs, PreCommitStep, predecessor_bridge, resolve};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

#[test]
fn the_predecessor_step_executes_the_current_source_rust_gate_once() {
    let scratch = Scratch::new("dispatch");
    let cargo = scratch.path().join("cargo");
    fs::write(
        &cargo,
        "#!/usr/bin/env sh\nprintf '%s\\n' \"$@\" > cargo.argv\nexit 23\n",
    )
    .unwrap();
    executable(&cargo);

    let command = declared_command();
    assert_eq!(command.label(), "Rust gate");
    assert_eq!(
        command.run(),
        [
            "cargo",
            "run",
            "--locked",
            "--package",
            "spice",
            "--",
            "gate",
            "pre-commit",
        ]
    );
    assert!(
        command.scopes().paths().is_empty(),
        "the bridge must run for every commit"
    );

    let status = Command::new(&cargo)
        .args(&command.run()[1..])
        .current_dir(scratch.path())
        .status()
        .expect("tracked predecessor step executes");
    assert_eq!(status.code(), Some(23), "the Rust gate status propagates");
    assert_eq!(
        fs::read_to_string(scratch.path().join("cargo.argv")).unwrap(),
        "run\n--locked\n--package\nspice\n--\ngate\npre-commit\n",
        "Cargo received more or different arguments"
    );
}

#[test]
fn a_missing_cargo_executable_refuses_without_another_route() {
    let scratch = Scratch::new("missing");
    let command = declared_command();
    let missing = scratch.path().join(&command.run()[0]);
    let error = Command::new(&missing)
        .args(&command.run()[1..])
        .current_dir(scratch.path())
        .output()
        .expect_err("the sole Cargo route must be absent");

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(
        missing.file_name().and_then(|name| name.to_str()),
        Some("cargo")
    );
    assert!(!scratch.path().join("cargo.argv").exists());
}

fn declared_command() -> PreCommitStep {
    let document = fs::read_to_string(workspace().join("spice.toml")).unwrap();
    let resolved = resolve(Inputs::new(
        Some(Document::new("spice.toml", &document)),
        None,
        &[],
    ))
    .expect("tracked policy resolves through the current registry");
    predecessor_bridge(resolved.root())
        .expect("Python builtins must be off with exactly one predecessor bridge")
        .clone()
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice crate has a workspace parent")
        .to_path_buf()
}

fn executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let nonce = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-hook-bridge-{}-{nonce}-{name}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
