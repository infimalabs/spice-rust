#![cfg(unix)]

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use spice::install::{InstallPreview, InstallReceipt, InstallStatus};
use wire::Outcome;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn installed_spice_repairs_relocation_with_a_surviving_earlier_linked_worktree() {
    let fixture = Fixture::new();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let original_config = image(&fixture.main.join(".git/config"));
    let original_link = image(&fixture.linked.join(".git"));

    let preview: InstallPreview = reply(invoke(source, &fixture.main, &["--json", "install"]));

    assert_eq!(image(&fixture.main.join(".git/config")), original_config);
    assert_eq!(image(&fixture.linked.join(".git")), original_link);
    assert!(!fixture.main.join(".git/.spice-install").exists());
    assert!(!fixture.main.join(".git/.spice-rs/install.json").exists());
    let installed: InstallReceipt = reply(invoke(
        source,
        &fixture.main,
        &[
            "--json",
            "install",
            &format!("--apply={}", preview.digest()),
        ],
    ));
    assert_eq!(installed.status(), InstallStatus::Complete);
    let old_hooks = hook_path(&fixture.main).expect("initial install selects hooks");

    fs::rename(&fixture.main, &fixture.moved).expect("relocate main worktree and common Git data");
    git(
        &fixture.moved,
        &["worktree", "repair", fixture.linked.to_str().unwrap()],
    );
    assert!(fixture.linked < fixture.moved);
    let installed_spice = fixture.moved.join(".git/.spice-install/bin/spice");
    let common = fixture.moved.join(".git");
    let before_preview = install_images(&common, &installed);
    let config_before_preview = image(&common.join("config"));
    let record_before_preview = image(&common.join(".spice-rs/install.json"));

    let refresh: InstallPreview = reply(invoke(
        &installed_spice,
        &fixture.moved,
        &["--json", "install"],
    ));

    assert_eq!(install_images(&common, &installed), before_preview);
    assert_eq!(image(&common.join("config")), config_before_preview);
    assert_eq!(
        image(&common.join(".spice-rs/install.json")),
        record_before_preview
    );
    let activation = refresh
        .plan()
        .hook_activation()
        .expect("relocation repair carries hook activation");
    let worktrees = activation
        .worktrees()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let canonical_linked = fs::canonicalize(&fixture.linked).expect("canonicalize linked worktree");
    let canonical_moved = fs::canonicalize(&fixture.moved).expect("canonicalize moved worktree");
    assert_eq!(
        worktrees,
        [
            canonical_linked.to_str().unwrap(),
            canonical_moved.to_str().unwrap()
        ]
    );
    assert_ne!(activation.hooks_path(), old_hooks);

    let repaired: InstallReceipt = reply(invoke(
        &installed_spice,
        &fixture.moved,
        &[
            "--json",
            "install",
            &format!("--apply={}", refresh.digest()),
        ],
    ));
    assert_eq!(repaired.status(), InstallStatus::Complete);
    assert_eq!(
        hook_path(&fixture.moved).as_deref(),
        Some(activation.hooks_path())
    );
    assert_eq!(
        hook_path(&fixture.linked).as_deref(),
        Some(activation.hooks_path())
    );

    let removed: InstallReceipt = reply(invoke(
        &installed_spice,
        &fixture.moved,
        &["--json", "install", "--unapply"],
    ));
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert!(hook_path(&fixture.moved).is_none());
    assert!(hook_path(&fixture.linked).is_none());
    assert!(!installed_spice.exists());
}

fn invoke(executable: &Path, worktree: &Path, arguments: &[&str]) -> Output {
    Command::new(executable)
        .args(arguments)
        .current_dir(worktree)
        .env_remove(steer::hook::INBOX)
        .env_remove(spice::install::TEST_EXECUTABLE)
        .output()
        .expect("launch Spice install process")
}

fn reply<T: DeserializeOwned + std::fmt::Debug>(output: Output) -> T {
    assert!(
        output.status.success(),
        "install failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stderr.is_empty());
    let outcome: Outcome<T> = wire::decode(&output.stdout).expect("decode install outcome");
    let Outcome::Reply(reply) = outcome else {
        panic!("install did not reply: {outcome:?}");
    };
    reply.body
}

fn hook_path(worktree: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/git")
        .args(["config", "--get", "core.hooksPath"])
        .current_dir(worktree)
        .output()
        .expect("read effective hook path");
    if output.status.code() == Some(1) {
        return None;
    }
    assert!(output.status.success());
    Some(
        String::from_utf8(output.stdout)
            .expect("hook path is UTF-8")
            .trim_ascii_end()
            .to_owned(),
    )
}

fn install_images(common: &Path, receipt: &InstallReceipt) -> Vec<FileImage> {
    receipt
        .plan()
        .operations()
        .iter()
        .map(|operation| image(&common.join(operation.path())))
        .collect()
}

#[derive(Debug, Eq, PartialEq)]
struct FileImage {
    device: u64,
    inode: u64,
    mode: u32,
    bytes: Vec<u8>,
}

fn image(path: &Path) -> FileImage {
    let metadata = fs::metadata(path).unwrap_or_else(|error| panic!("inspect {path:?}: {error}"));
    FileImage {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        bytes: fs::read(path).unwrap_or_else(|error| panic!("read {path:?}: {error}")),
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("launch fixture Git");
    assert!(
        output.status.success(),
        "Git failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

struct Fixture {
    root: PathBuf,
    main: PathBuf,
    linked: PathBuf,
    moved: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-linked-relocation-{}-{sequence}",
            std::process::id()
        ));
        let main = root.join("a-main");
        let linked = root.join("z-linked");
        let moved = root.join("zz-moved");
        fs::create_dir_all(&main).expect("create relocation fixture");
        git(&main, &["init", "-q", "-b", "main"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Review Fixture",
                "-c",
                "user.email=review@example.invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        );
        git(
            &main,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "worktree",
                "add",
                "-q",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        Self {
            root,
            main,
            linked,
            moved,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root)
            && !std::thread::panicking()
        {
            panic!("remove relocation fixture {:?}: {error}", self.root);
        }
    }
}
