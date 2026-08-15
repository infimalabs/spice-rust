use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest as _, Sha256};

mod real_process;

struct Fixture {
    root: PathBuf,
    bootstrap: PathBuf,
    first: PathBuf,
    second: PathBuf,
    cargo: PathBuf,
    counter: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-cargo-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let bootstrap = root.join("cargo-home");
        let first = root.join("first");
        let second = root.join("second");
        let package = root.join("package");
        let cargo = root.join("native-cargo");
        let counter = root.join("seed-count");
        fs::create_dir_all(package.join("src")).expect("create package fixture");
        fs::create_dir(&bootstrap).expect("create bootstrap Cargo home");
        fs::create_dir(&first).expect("create first worktree");

        let package_manifest =
            b"[package]\nname = \"fixture-dependency\"\nversion = \"1.0.0\"\nedition = \"2024\"\n";
        let package_source = b"pub fn answer() -> u8 { 42 }\n";
        fs::write(package.join("Cargo.toml"), package_manifest).expect("write package manifest");
        fs::write(package.join("src/lib.rs"), package_source).expect("write package source");
        fs::write(
            package.join(".cargo-checksum.json"),
            serde_json::to_vec(&serde_json::json!({
                "files": {
                    "Cargo.toml": sha256(package_manifest),
                    "src/lib.rs": sha256(package_source),
                },
                "package": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            }))
            .expect("serialize checksum fixture"),
        )
        .expect("write package checksum");
        fs::write(
            first.join("Cargo.toml"),
            "[package]\nname = \"fixture-root\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write repository manifest");
        fs::write(
            first.join("Cargo.lock"),
            concat!(
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"fixture-dependency\"\n",
                "version = \"1.0.0\"\n",
                "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
                "checksum = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
            ),
        )
        .expect("write exact lockfile");
        git(&first, &["init"]);
        git(&first, &["config", "user.name", "Spice Test"]);
        git(&first, &["config", "user.email", "spice@example.invalid"]);
        git(&first, &["config", "core.hooksPath", "/dev/null"]);
        git(&first, &["add", "Cargo.toml", "Cargo.lock"]);
        git(&first, &["commit", "-m", "fixture"]);
        git(
            &first,
            &[
                "worktree",
                "add",
                "-b",
                "fixture-second",
                second.to_str().expect("UTF-8 fixture path"),
            ],
        );

        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "if [ \"${{1-}}\" = vendor ]; then\n",
                "  destination=\n",
                "  for argument in \"$@\"; do destination=$argument; done\n",
                "  printf 'seed\\n' >> {}\n",
                "  sleep 0.1\n",
                "  cp -R {} \"$destination/fixture-dependency-1.0.0\"\n",
                "  printf '[source.crates-io]\\nreplace-with = \"vendored-sources\"\\n\\n",
                "[source.vendored-sources]\\ndirectory = \"%s\"\\n' \"$destination\"\n",
                "  exit 0\n",
                "fi\n",
                "test -f \"$CARGO_HOME/config.toml\"\n",
                "grep -q 'offline = true' \"$CARGO_HOME/config.toml\"\n",
                "test ! -e \"$CARGO_HOME/registry\"\n",
                "test ! -e \"$CARGO_HOME/git\"\n",
                "printf 'private-db\\n' > \"$CARGO_HOME/.global-cache\"\n",
                "printf '%s\\n' \"$CARGO_HOME\"\n",
                "if [ \"${{1-}}\" = exit-23 ]; then exit 23; fi\n",
            ),
            shell_word(&counter),
            shell_word(&package),
        );
        fs::write(&cargo, script).expect("write fake native Cargo");
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700))
            .expect("make fake Cargo executable");

        Self {
            root,
            bootstrap,
            first,
            second,
            cargo,
            counter,
        }
    }

    fn command(&self, worktree: &Path, argument: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(["dev", "cargo", "--", argument])
            .current_dir(worktree)
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &self.bootstrap,
            )
            .env(spice::cargo_isolation::NATIVE_CARGO_VARIABLE, &self.cargo)
            .env_remove("CARGO_HOME")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn two_linked_worktrees_converge_cold_then_validate_in_private_homes() {
    let fixture = Fixture::new();
    let first = fixture
        .command(&fixture.first, "validate")
        .spawn()
        .expect("launch first isolated Cargo");
    let second = fixture
        .command(&fixture.second, "validate")
        .spawn()
        .expect("launch second isolated Cargo");
    let first = first.wait_with_output().expect("wait for first Cargo");
    let second = second.wait_with_output().expect("wait for second Cargo");

    assert_success(&first);
    assert_success(&second);
    let first_home = spoken_home(&first);
    let second_home = spoken_home(&second);
    assert_ne!(first_home, second_home);
    assert_eq!(mode(&first_home), 0o700);
    assert_eq!(mode(&second_home), 0o700);
    assert_ne!(
        fs::metadata(first_home.join(".global-cache"))
            .expect("first private database")
            .ino(),
        fs::metadata(second_home.join(".global-cache"))
            .expect("second private database")
            .ino(),
    );
    assert!(tree_bytes(&first_home) <= 1024 * 1024);
    assert!(tree_bytes(&second_home) <= 1024 * 1024);
    assert_eq!(
        fs::read_to_string(&fixture.counter)
            .expect("cold seed counter")
            .lines()
            .count(),
        1,
    );
    for output in [&first, &second] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("Blocking waiting for file lock"));
        assert!(!stderr.contains("package cache lock"));
    }

    let first_source = configured_source(&first_home);
    let second_source = configured_source(&second_home);
    assert_eq!(first_source, second_source);
    let packages = fixture.bootstrap.join("spice/source-store-v2/packages");
    assert_eq!(normal_directories(&packages), 1);
    let linked = fs::read_dir(first_source)
        .expect("read generation view")
        .next()
        .expect("one generation package")
        .expect("read generation package entry")
        .path();
    assert!(
        fs::symlink_metadata(linked)
            .expect("linked package metadata")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn native_cargo_exit_status_is_preserved() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.first, "exit-23")
        .output()
        .expect("run status fixture");
    assert_eq!(output.status.code(), Some(23));
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "isolated Cargo failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn spoken_home(output: &Output) -> PathBuf {
    PathBuf::from(
        String::from_utf8(output.stdout.clone())
            .expect("fake Cargo emits UTF-8")
            .trim(),
    )
}

fn configured_source(home: &Path) -> PathBuf {
    let document: toml::Value = toml::from_str(
        &fs::read_to_string(home.join("config.toml")).expect("read private Cargo config"),
    )
    .expect("parse private Cargo config");
    PathBuf::from(
        document["source"]["vendored-sources"]["directory"]
            .as_str()
            .expect("generated source directory"),
    )
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("launch fixture Git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn normal_directories(path: &Path) -> usize {
    fs::read_dir(path)
        .expect("read owned directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count()
}

fn tree_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .expect("read private home")
        .filter_map(Result::ok)
        .map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).expect("measure private home entry");
            if metadata.is_dir() {
                tree_bytes(&entry.path())
            } else {
                metadata.len()
            }
        })
        .sum()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn shell_word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("read fixture mode")
        .permissions()
        .mode()
        & 0o777
}

fn make_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    } else {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}
