use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const LARGE_BINARY_BYTES: usize = 1024 * 1024;
const MAX_COMPACT_PLAN_BYTES: usize = 32 * 1024;

#[test]
fn planning_is_canonical_digest_bound_and_side_effect_free() {
    let scratch = Scratch::new("plan");
    let repository = scratch.repository();
    let binary_bytes = vec![0x5a; LARGE_BINARY_BYTES];
    let binary = scratch.binary(&binary_bytes);

    let first = plan(&repository, Some(&binary)).expect("derive install plan");
    let second = plan(&repository, Some(&binary)).expect("derive same install plan");

    assert_eq!(first, second);
    assert_eq!(
        first.canonical_bytes().unwrap(),
        second.canonical_bytes().unwrap()
    );
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    assert!(first.canonical_bytes().unwrap().len() < MAX_COMPACT_PLAN_BYTES);
    assert_eq!(
        first
            .operations()
            .iter()
            .map(Operation::asset)
            .collect::<Vec<_>>(),
        [
            Asset::Exclusions,
            Asset::Configuration,
            Asset::HookEntry,
            Asset::HookStatic,
            Asset::Skill,
            Asset::Binary,
        ]
    );
    assert_eq!(fs::read(scratch.exclude()).unwrap(), b"# local ignore\n");
    assert!(!record_path(&repository).exists());
    assert!(!scratch.join(ENTRY_PATH).exists());
    assert!(!scratch.join(SKILL_PATH).exists());
}

#[test]
fn apply_is_idempotent_preserves_python_and_unapply_is_exact() {
    let scratch = Scratch::new("round-trip");
    let repository = scratch.repository();
    let python = scratch.executable(
        "bin/spice",
        b"#!/usr/bin/env python3\nprint('python-harness')\n",
    );
    let binary = scratch.binary(b"#!/bin/sh\nprintf 'rust-appliance\\n'\n");
    let before_python = fs::read(&python).unwrap();
    let before_exclude = fs::read(scratch.exclude()).unwrap();

    let planned = plan(&repository, Some(&binary)).expect("plan install");
    let digest = planned.digest().unwrap();
    let first = apply(&repository, Some(&binary), &digest).expect("apply exact plan");
    let installed_images = images(&repository, first.plan());
    let repeated = apply(&repository, Some(&binary), &digest).expect("repeat exact apply");

    assert_eq!(first.status(), InstallStatus::Complete);
    assert_eq!(first.completed(), planned.operations().len());
    assert_eq!(first, repeated);
    assert_eq!(installed_images, images(&repository, repeated.plan()));
    assert_eq!(fs::read(&python).unwrap(), before_python);
    assert_eq!(run(&python), b"python-harness\n");
    assert_eq!(run(&scratch.join(BINARY_PATH)), b"rust-appliance\n");
    assert_eq!(
        fs::read(scratch.common(CONFIG_PATH)).unwrap(),
        STARTER_CONFIG.as_bytes()
    );
    assert!(!scratch.join(CONFIG_PATH).exists());
    crate::config::load_repository(&repository, &[]).expect("starter configuration loads");
    assert!(
        fs::read_to_string(scratch.exclude())
            .unwrap()
            .contains(EXCLUDE_BLOCK)
    );
    assert_eq!(fs::read_to_string(scratch.join(SKILL_PATH)).unwrap(), SKILL);

    let removed = unapply(&repository).expect("reverse exact install");
    let repeated_remove = unapply(&repository).expect("repeat exact reversal");
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert_eq!(removed, repeated_remove);
    assert_eq!(fs::read(scratch.exclude()).unwrap(), before_exclude);
    assert_eq!(fs::read(&python).unwrap(), before_python);
    for operation in planned.operations() {
        let observed = observe(&operation.destination(&repository))
            .unwrap()
            .map(|observed| observed.image);
        assert_eq!(observed, operation.previous().cloned());
    }
    assert!(record_path(&repository).is_file());
}

#[test]
fn preserved_files_keep_their_bytes_and_modes() {
    let scratch = Scratch::new("preserve");
    let repository = scratch.repository();
    let config = scratch.common_file(CONFIG_PATH, b"[operator]\nmode = 'kept'\n");
    set_test_mode(&scratch.exclude(), 0o600);
    set_test_mode(&config, 0o640);

    let planned = plan(&repository, None).unwrap();
    let receipt = apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert_eq!(fs::read(&config).unwrap(), b"[operator]\nmode = 'kept'\n");
    assert_eq!(mode(&scratch.exclude()), 0o600);
    assert_eq!(mode(&config), 0o640);

    unapply(&repository).unwrap();
    assert_eq!(
        receipt.plan().operations()[1].previous().unwrap().mode(),
        0o640
    );
    assert_eq!(fs::read(&config).unwrap(), b"[operator]\nmode = 'kept'\n");
    assert_eq!(mode(&scratch.exclude()), 0o600);
    assert_eq!(mode(&config), 0o640);
}

#[test]
fn stale_plan_refuses_before_record_or_target_mutation() {
    let scratch = Scratch::new("stale");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    let stale = planned.digest().unwrap();
    fs::write(scratch.exclude(), b"operator changed this\n").unwrap();

    let error = apply(&repository, None, &stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(
        fs::read(scratch.exclude()).unwrap(),
        b"operator changed this\n"
    );
    assert!(!record_path(&repository).exists());
    assert!(!scratch.common(CONFIG_PATH).exists());
    assert!(!scratch.join(ENTRY_PATH).exists());
}

#[test]
fn changed_optional_binary_refuses_the_planned_digest() {
    let scratch = Scratch::new("binary-stale");
    let repository = scratch.repository();
    let binary = scratch.binary(b"old binary bytes");
    let stale = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    fs::write(&binary, b"new binary bytes").unwrap();

    let error = apply(&repository, Some(&binary), &stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert!(!record_path(&repository).exists());
    assert!(!scratch.join(BINARY_PATH).exists());
}

#[test]
fn apply_resumes_a_write_interrupted_before_progress_publication() {
    let scratch = Scratch::new("apply-resume");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    let digest = planned.digest().unwrap();

    let error = apply_with_interruption(&repository, None, &digest, Some(1)).unwrap_err();
    assert!(matches!(error, InstallError::Interrupted { operation: 1 }));
    let interrupted = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(interrupted.status, InstallStatus::Applying);
    assert_eq!(interrupted.completed, 1);
    assert_eq!(plan(&repository, None).unwrap().digest().unwrap(), digest);

    let completed = apply(&repository, None, &digest).unwrap();
    assert_eq!(completed.status(), InstallStatus::Complete);
    assert_eq!(completed.completed(), planned.operations().len());
    assert!(planned.operations().iter().all(|operation| {
        let observed = observe(&operation.destination(&repository)).unwrap();
        matches_generated(observed.as_ref(), operation)
    }));
}

#[test]
fn unapply_resumes_a_write_interrupted_before_progress_publication() {
    let scratch = Scratch::new("unapply-resume");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();

    let error = unapply_with_interruption(&repository, Some(3)).unwrap_err();
    assert!(matches!(error, InstallError::Interrupted { operation: 3 }));
    let interrupted = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(interrupted.status, InstallStatus::Unapplying);
    assert_eq!(interrupted.completed, 1);
    assert_eq!(
        plan(&repository, None).unwrap().digest().unwrap(),
        planned.digest().unwrap()
    );

    let completed = unapply(&repository).unwrap();
    assert_eq!(completed.status(), InstallStatus::Uninstalled);
    assert!(planned.operations().iter().all(|operation| {
        observe(&operation.destination(&repository))
            .unwrap()
            .map(|observed| observed.image)
            == operation.previous().cloned()
    }));
}

#[test]
fn operator_drift_refuses_unapply_before_any_reversal() {
    let scratch = Scratch::new("drift");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    let generated = images(&repository, &planned);
    fs::write(scratch.join(ENTRY_PATH), b"operator-owned replacement\n").unwrap();

    let error = unapply(&repository).unwrap_err();
    assert!(matches!(
        error,
        InstallError::Drift { path, .. } if path == ENTRY_PATH
    ));
    let record = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(record.status, InstallStatus::Complete);
    for (index, operation) in planned.operations().iter().enumerate() {
        if operation.path() != ENTRY_PATH {
            assert_eq!(
                observe(&operation.destination(&repository)).unwrap(),
                generated[index]
            );
        }
    }
}

#[test]
fn malformed_record_and_divergent_exclusions_refuse_without_mutation() {
    let malformed = Scratch::new("malformed");
    let repository = malformed.repository();
    fs::create_dir_all(record_path(&repository).parent().unwrap()).unwrap();
    fs::write(record_path(&repository), b"{\"unexpected\":true}\n").unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::Decode(_)));
    assert!(!malformed.join(ENTRY_PATH).exists());

    let divergent = Scratch::new("divergent");
    let repository = divergent.repository();
    fs::write(divergent.exclude(), format!("{EXCLUDE_BEGIN}\nwrong\n")).unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::DivergentExclusions));
    assert!(!record_path(&repository).exists());
    assert!(!divergent.common(CONFIG_PATH).exists());

    let duplicate = Scratch::new("duplicate-block");
    let repository = duplicate.repository();
    fs::write(
        duplicate.exclude(),
        format!("{EXCLUDE_BLOCK}{EXCLUDE_BLOCK}"),
    )
    .unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::DivergentExclusions));
    assert!(!record_path(&repository).exists());
}

#[test]
fn legacy_record_names_its_owning_release_and_refuses_without_mutation() {
    let scratch = Scratch::new("legacy-record");
    let repository = scratch.repository();
    let legacy = repository.common_dir().join(LEGACY_RECORD_PATH);
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, b"legacy record bytes\n").unwrap();
    let before_exclude = fs::read(scratch.exclude()).unwrap();

    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        &error,
        InstallError::LegacyRecord(path) if path == &legacy
    ));
    assert!(error.to_string().contains("SURFACE-1kJQkpFs"));
    assert_eq!(fs::read(scratch.exclude()).unwrap(), before_exclude);
    assert!(!record_path(&repository).exists());
    assert!(!scratch.common(CONFIG_PATH).exists());
    assert!(!scratch.join(ENTRY_PATH).exists());
}

#[test]
fn record_digest_tampering_refuses_before_unapply_mutation() {
    let scratch = Scratch::new("record-digest");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    let generated = images(&repository, &planned);
    let mut record = read_record(&repository).unwrap().unwrap().record;
    record.plan_digest = Digest::compute(Domain::InstallPlan, b"tampered");
    fs::write(record_path(&repository), wire::encode(&record).unwrap()).unwrap();

    let error = unapply(&repository).unwrap_err();
    assert!(matches!(error, InstallError::InvalidRecord(_)));
    assert_eq!(images(&repository, &planned), generated);
}

#[cfg(unix)]
#[test]
fn planning_binds_internal_symlinked_install_parents_and_refuses_escape() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("symlink");
    let repository = scratch.repository();
    let outside = scratch.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, scratch.join(".agents")).unwrap();

    let planned = plan(&repository, None).unwrap();
    let skill = planned
        .operations()
        .iter()
        .find(|operation| operation.asset() == Asset::Skill)
        .unwrap();
    assert_eq!(skill.destination, "outside/skills/spice-rust/SKILL.md");
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(outside.join("skills/spice-rust/SKILL.md").is_file());
    unapply(&repository).unwrap();

    let escaped = Scratch::new("symlink-escape");
    let repository = escaped.repository();
    let external = escaped.0.with_extension("external");
    fs::create_dir(&external).unwrap();
    let canonical_external = fs::canonicalize(&external).unwrap();
    symlink(&external, escaped.join(".agents")).unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        error,
        InstallError::PathOutsideRoot(path) if path == canonical_external
    ));
    assert!(fs::read_dir(&external).unwrap().next().is_none());
    fs::remove_dir(external).unwrap();
}

fn images(repository: &Repository, plan: &InstallPlan) -> Vec<Option<ObservedFile>> {
    plan.operations()
        .iter()
        .map(|operation| observe(&operation.destination(repository)).unwrap())
        .collect()
}

fn run(path: &Path) -> Vec<u8> {
    let output = Command::new(path)
        .output()
        .expect("invoke fixture executable");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    output.stdout
}

#[cfg(unix)]
fn set_test_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_test_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode(_path: &Path) -> u32 {
    DATA_MODE
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-{label}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    fs::create_dir_all(root.join(".git/info")).unwrap();
                    fs::write(root.join(".git/info/exclude"), b"# local ignore\n").unwrap();
                    return Self(root);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create install fixture {}: {error}", root.display()),
            }
        }
        panic!("no unique install fixture path was available");
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.0).expect("discover fixture repository")
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    fn exclude(&self) -> PathBuf {
        self.0.join(".git/info/exclude")
    }

    fn common(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(".git").join(relative)
    }

    fn common_file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.common(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn binary(&self, bytes: &[u8]) -> PathBuf {
        self.executable("build/spice-rust", bytes)
    }

    fn executable(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        set_test_mode(&path, EXECUTABLE_MODE);
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && !std::thread::panicking()
        {
            panic!("remove install fixture {}: {error}", self.0.display());
        }
    }
}
