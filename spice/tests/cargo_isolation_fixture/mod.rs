use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use super::cargo_isolation_cleanup::make_writable;
use super::cargo_isolation_support::{git, sha256, shell_word, write_executable};

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) bootstrap: PathBuf,
    pub(super) first: PathBuf,
    pub(super) second: PathBuf,
    pub(super) cargo: PathBuf,
    pub(super) rustc: PathBuf,
    pub(super) rustdoc: PathBuf,
    pub(super) hostile: PathBuf,
    pub(super) hostile_marker: PathBuf,
    pub(super) counter: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
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
        let rustc = root.join("native-rustc");
        let rustdoc = root.join("native-rustdoc");
        let hostile = root.join("hostile-bin");
        let hostile_marker = root.join("hostile-tool-invoked");
        let counter = root.join("seed-count");
        let fixture = Self {
            root,
            bootstrap,
            first,
            second,
            cargo,
            rustc,
            rustdoc,
            hostile,
            hostile_marker,
            counter,
        };
        fixture.prepare(&package);
        fixture
    }

    fn prepare(&self, package: &Path) {
        fs::create_dir_all(package.join("src")).expect("create package fixture");
        fs::create_dir(&self.bootstrap).expect("create bootstrap Cargo home");
        fs::create_dir(&self.first).expect("create first worktree");
        fs::create_dir(&self.hostile).expect("create hostile tool directory");
        self.seed_package(package);
        self.seed_repository();
        self.seed_toolchain(package);
    }

    fn seed_package(&self, package: &Path) {
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
    }

    fn seed_repository(&self) {
        fs::write(
            self.first.join("Cargo.toml"),
            "[package]\nname = \"fixture-root\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write repository manifest");
        fs::write(
            self.first.join("Cargo.lock"),
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
        git(&self.first, &["init"]);
        git(&self.first, &["config", "user.name", "Spice Test"]);
        git(
            &self.first,
            &["config", "user.email", "spice@example.invalid"],
        );
        git(&self.first, &["config", "core.hooksPath", "/dev/null"]);
        git(&self.first, &["add", "Cargo.toml", "Cargo.lock"]);
        git(&self.first, &["commit", "-m", "fixture"]);
        git(
            &self.first,
            &[
                "worktree",
                "add",
                "-b",
                "fixture-second",
                self.second.to_str().expect("UTF-8 fixture path"),
            ],
        );
    }

    fn seed_toolchain(&self, package: &Path) {
        let canonical_root = fs::canonicalize(&self.root).expect("canonicalize fixture root");
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
                "test \"$SPICE_RS_NATIVE_CARGO\" = {}\n",
                "test \"$SPICE_RS_NATIVE_RUSTC\" = {}\n",
                "test \"$SPICE_RS_NATIVE_RUSTDOC\" = {}\n",
                "test \"$RUSTC\" = \"$SPICE_RS_NATIVE_RUSTC\"\n",
                "test \"$RUSTDOC\" = \"$SPICE_RS_NATIVE_RUSTDOC\"\n",
                "test \"$(\"$RUSTC\")\" = exact-rustc\n",
                "test \"$(\"$RUSTDOC\")\" = exact-rustdoc\n",
                "test -f \"$CARGO_HOME/config.toml\"\n",
                "grep -q 'offline = true' \"$CARGO_HOME/config.toml\"\n",
                "test ! -e \"$CARGO_HOME/registry\"\n",
                "test ! -e \"$CARGO_HOME/git\"\n",
                "printf 'private-db\\n' > \"$CARGO_HOME/.global-cache\"\n",
                "printf '%s\\n' \"$CARGO_HOME\"\n",
                "if [ \"${{1-}}\" = exit-23 ]; then exit 23; fi\n",
            ),
            shell_word(&self.counter),
            shell_word(package),
            shell_word(&canonical_root.join("native-cargo")),
            shell_word(&canonical_root.join("native-rustc")),
            shell_word(&canonical_root.join("native-rustdoc")),
        );
        fs::write(&self.cargo, script).expect("write fake native Cargo");
        fs::set_permissions(&self.cargo, fs::Permissions::from_mode(0o700))
            .expect("make fake Cargo executable");
        write_executable(&self.rustc, "#!/bin/sh\nprintf 'exact-rustc\\n'\n");
        write_executable(&self.rustdoc, "#!/bin/sh\nprintf 'exact-rustdoc\\n'\n");
        for name in ["cargo", "rustc", "rustdoc"] {
            write_executable(
                &self.hostile.join(name),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' invoked >> {}\nexit 91\n",
                    shell_word(&self.hostile_marker),
                ),
            );
        }
    }

    pub(super) fn command(&self, worktree: &Path, argument: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(["dev", "cargo", "--", argument])
            .current_dir(worktree)
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &self.bootstrap,
            )
            .env(spice::cargo_isolation::NATIVE_CARGO_VARIABLE, &self.cargo)
            .env(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE, &self.rustc)
            .env(
                spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
                &self.rustdoc,
            )
            .env(
                "PATH",
                std::env::join_paths([
                    self.hostile.clone(),
                    PathBuf::from("/usr/bin"),
                    PathBuf::from("/bin"),
                ])
                .expect("join deterministic hostile PATH"),
            )
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
