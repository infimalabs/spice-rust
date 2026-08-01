#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{InstallStatus, apply, plan, unapply};
use spice::repository::Repository;

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
        fs::metadata(scratch.join(".git/.spice/rust/install.json"))
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
