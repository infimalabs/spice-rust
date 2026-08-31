#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{
    InstallError, InstallStatus, apply, installed_bash_env, installed_binary, installed_git_hooks,
    installed_skill, installed_spice_wrapper, installed_zdotdir, plan, unapply,
};
use spice::repository::Repository;

#[path = "support/git_repository.rs"]
mod git_repository;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const MAX_COMPACT_RECORD_BYTES: u64 = 64 * 1024;

#[test]
fn installed_spice_is_the_one_repository_local_executable() {
    let scratch = Scratch::new("appliance");
    let repository = Repository::discover(scratch.path()).unwrap();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));

    let planned = plan(&repository, Some(source)).unwrap();
    let receipt = apply(&repository, Some(source), &planned.digest().unwrap()).unwrap();
    let installed = installed_binary(&repository);

    assert_eq!(receipt.status(), InstallStatus::Complete);
    let help = run(&installed, &["--help"]);
    assert!(
        String::from_utf8(help)
            .unwrap()
            .contains("Repository-local coordination appliance")
    );
    assert_eq!(
        fs::metadata(&installed).unwrap().len(),
        fs::metadata(source).unwrap().len()
    );
    assert!(installed_skill(&repository).is_file());
    let wrapper = installed_spice_wrapper(&repository);
    assert_eq!(
        fs::read(&wrapper).unwrap(),
        steer::hook::STAGES[4].bytes().as_bytes()
    );
    assert_eq!(
        fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(installed_zdotdir(&repository).join(".zshenv")).unwrap(),
        steer::hook::STAGES[2].bytes().as_bytes()
    );
    assert_eq!(
        fs::read(installed_bash_env(&repository)).unwrap(),
        steer::hook::STAGES[3].bytes().as_bytes()
    );
    let receipt = fs::metadata(scratch.join(".git/.spice-rs/install.json")).unwrap();
    assert!(receipt.len() < MAX_COMPACT_RECORD_BYTES);
    assert_eq!(receipt.permissions().mode() & 0o777, 0o600);
    assert!(!scratch.join(".git/.spice/rust/install.json").exists());

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed.exists());
}

#[test]
fn linked_worktrees_share_one_common_install_and_receipt() {
    let scratch = Scratch::new("linked");
    let common = scratch.join(".git");
    let linked = scratch.join("linked");
    git_repository::link(scratch.path(), &linked, "linked");
    let first_repository = Repository::discover(scratch.path()).unwrap();
    let linked_repository = Repository::discover(&linked).unwrap();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));

    let planned = plan(&first_repository, Some(source)).unwrap();
    let installed = apply(&first_repository, Some(source), &planned.digest().unwrap()).unwrap();
    let linked_plan = plan(&linked_repository, Some(source)).unwrap();
    let linked_install = apply(
        &linked_repository,
        Some(source),
        &linked_plan.digest().unwrap(),
    )
    .unwrap();

    assert_eq!(installed.status(), InstallStatus::Complete);
    assert_eq!(linked_install, installed);
    assert_eq!(linked_plan, planned);
    assert_eq!(
        installed_binary(&first_repository),
        installed_binary(&linked_repository)
    );
    assert_eq!(
        installed_git_hooks(&first_repository),
        installed_git_hooks(&linked_repository)
    );
    assert!(installed_binary(&linked_repository).is_file());
    assert!(installed_skill(&linked_repository).is_file());
    assert!(installed_spice_wrapper(&linked_repository).is_file());
    assert!(installed_git_hooks(&linked_repository).is_dir());
    assert!(common.join(".spice-rs/install.json").is_file());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(common.join(".spice-rs/install.json")).unwrap()).unwrap();
    assert!(record["repository"].get("worktree").is_none());
    assert!(record["plan"]["repository"].get("worktree").is_none());
    assert!(!scratch.join(".spice-install").exists());
    assert!(!linked.join(".spice-install").exists());

    assert_eq!(
        unapply(&linked_repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed_binary(&first_repository).exists());
    assert!(common.join(".spice-rs/install.json").is_file());
}

#[test]
fn stable_and_swapped_parent_symlinks_bind_one_physical_destination() {
    let stable = Scratch::new("stable-symlink");
    let repository = stable.repository();
    let target = repository.common_dir().join("appliance-assets");
    fs::create_dir(&target).unwrap();
    symlink(&target, repository.common_dir().join(".spice-install")).unwrap();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(target.join("agents/skills/spice/SKILL.md").is_file());
    unapply(&repository).unwrap();
    assert!(!target.join("agents/skills/spice/SKILL.md").exists());

    let swapped = Scratch::new("swapped-symlink");
    let repository = swapped.repository();
    let first = repository.common_dir().join("appliance-assets-a");
    let second = repository.common_dir().join("appliance-assets-b");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    symlink(&first, repository.common_dir().join(".spice-install")).unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::remove_file(repository.common_dir().join(".spice-install")).unwrap();
    symlink(&second, repository.common_dir().join(".spice-install")).unwrap();
    let before = swapped.snapshot();
    let error = apply(&repository, None, &stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(swapped.snapshot(), before);
    assert!(!swapped.record().exists());
}

#[test]
fn parent_creation_replacement_and_same_byte_destination_drift_are_inert() {
    let created = Scratch::new("created-parent");
    let repository = created.repository();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::create_dir(repository.common_dir().join(".spice-install")).unwrap();
    assert_stale_apply_is_inert(&created, &repository, &stale);

    let replaced_parent = Scratch::new("replaced-parent");
    let repository = replaced_parent.repository();
    let parent = repository
        .common_dir()
        .join(".spice-install/agents/skills/spice");
    fs::create_dir_all(&parent).unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::rename(
        &parent,
        repository
            .common_dir()
            .join(".spice-install/agents/skills/prior-parent"),
    )
    .unwrap();
    fs::create_dir(&parent).unwrap();
    assert_stale_apply_is_inert(&replaced_parent, &repository, &stale);

    let replaced_file = Scratch::new("replaced-file");
    let repository = replaced_file.repository();
    let skill = repository
        .common_dir()
        .join(".spice-install/agents/skills/spice/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, b"operator skill\n").unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    let replacement = replaced_file.join("replacement-skill");
    fs::write(&replacement, fs::read(&skill).unwrap()).unwrap();
    fs::rename(replacement, skill).unwrap();
    assert_stale_apply_is_inert(&replaced_file, &repository, &stale);
}

#[test]
fn copied_corrupt_and_noncanonical_records_are_inert() {
    let source = applied("record-source");
    let copied = Scratch::new("record-copied");
    fs::create_dir_all(copied.record().parent().unwrap()).unwrap();
    fs::copy(source.record(), copied.record()).unwrap();
    assert_bad_record_is_inert(&copied);

    let corrupt = applied("record-corrupt");
    fs::write(corrupt.record(), b"{not-json").unwrap();
    assert_bad_record_is_inert(&corrupt);

    let noncanonical = applied("record-noncanonical");
    let mut bytes = fs::read(noncanonical.record()).unwrap();
    bytes.push(b'\n');
    fs::write(noncanonical.record(), bytes).unwrap();
    assert_bad_record_is_inert(&noncanonical);
}

#[test]
fn arbitrary_duplicate_reordered_and_mismatched_records_are_inert() {
    tamper_record("record-out-root", |record| {
        record["plan"]["operations"][0]["destination"] = "../outside".into();
    });
    tamper_record("record-duplicate", |record| {
        record["destinations"][1] = record["destinations"][0].clone();
    });
    tamper_record("record-reordered", |record| {
        record["plan"]["operations"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
    });
    tamper_record("record-plan-mismatch", |record| {
        record["plan_digest"] = record["plan"]["operations"][0]["generated"]["digest"].clone();
    });
    tamper_record("record-reversal", |record| {
        record["reversals"][0] = record["reversals"][1].clone();
    });
    tamper_record("record-old-version", |record| {
        record["version"] = 1.into();
    });
    tamper_record("record-expanded-rollback", |record| {
        record["plan"]["rollback_record"] = serde_json::json!({});
    });
}

#[test]
fn same_byte_installed_file_replacement_unapplies_while_parent_replacement_refuses_inertly() {
    let replaced_file = applied("installed-file-generation");
    let repository = replaced_file.repository();
    let entry = repository
        .common_dir()
        .join(".spice-install/hooks/entry.sh");
    let replacement = replaced_file.join("replacement-entry");
    fs::write(&replacement, fs::read(&entry).unwrap()).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(replacement, entry).unwrap();
    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!replaced_file.join(".spice/hooks/entry.sh").exists());

    let replaced_parent = applied("installed-parent-generation");
    let repository = replaced_parent.repository();
    let hooks = repository.common_dir().join(".spice-install/hooks");
    let prior = repository.common_dir().join(".spice-install/prior-hooks");
    fs::rename(&hooks, &prior).unwrap();
    fs::create_dir(&hooks).unwrap();
    fs::hard_link(prior.join("entry.sh"), hooks.join("entry.sh")).unwrap();
    fs::create_dir(hooks.join("static")).unwrap();
    fs::hard_link(prior.join("static/stage.sh"), hooks.join("static/stage.sh")).unwrap();
    let before = replaced_parent.snapshot();
    assert!(matches!(
        unapply(&repository),
        Err(InstallError::ParentChanged(_) | InstallError::Drift { .. })
    ));
    assert_eq!(replaced_parent.snapshot(), before);
}

fn assert_stale_apply_is_inert(scratch: &Scratch, repository: &Repository, stale: &wire::Digest) {
    let before = scratch.snapshot();
    let error = apply(repository, None, stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(scratch.snapshot(), before);
    assert!(!scratch.record().exists());
}

fn applied(label: &str) -> Scratch {
    let scratch = Scratch::new(label);
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    scratch
}

fn tamper_record(label: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let scratch = applied(label);
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.record()).unwrap()).unwrap();
    edit(&mut record);
    fs::write(scratch.record(), serde_json::to_vec(&record).unwrap()).unwrap();
    assert_bad_record_is_inert(&scratch);
}

fn assert_bad_record_is_inert(scratch: &Scratch) {
    let repository = scratch.repository();
    let before = scratch.snapshot();
    assert!(unapply(&repository).is_err());
    assert_eq!(scratch.snapshot(), before);
}

fn run(path: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(path).args(arguments).output().unwrap();
    assert!(output.status.success(), "{} failed", path.display());
    assert!(output.stderr.is_empty(), "{} wrote stderr", path.display());
    output.stdout
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-{label}-{}-{sequence}",
                std::process::id(),
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    git_repository::initialize(&root);
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

    fn repository(&self) -> Repository {
        Repository::discover(self.path()).unwrap()
    }

    fn record(&self) -> PathBuf {
        self.join(".git/.spice-rs/install.json")
    }

    fn snapshot(&self) -> Snapshot {
        let mut entries = Vec::new();
        snapshot_walk(self.path(), self.path(), &mut entries);
        Snapshot(entries)
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot(Vec<(PathBuf, u32, Node)>);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn snapshot_walk(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, u32, Node)>) {
    let mut children = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        let metadata = fs::symlink_metadata(&child).unwrap();
        let relative = child.strip_prefix(root).unwrap().to_path_buf();
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.file_type().is_symlink() {
            entries.push((
                relative,
                mode,
                Node::Symlink(fs::read_link(&child).unwrap()),
            ));
        } else if metadata.is_dir() {
            entries.push((relative, mode, Node::Directory));
            snapshot_walk(root, &child, entries);
        } else {
            entries.push((relative, mode, Node::File(fs::read(&child).unwrap())));
        }
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
