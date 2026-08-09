use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

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
            Asset::GitPreCommit,
            Asset::GitCommitMessage,
            Asset::GitReferenceTransaction,
            Asset::HookEntry,
            Asset::HookStatic,
            Asset::Skill,
            Asset::Binary,
        ]
    );
    assert_eq!(fs::read(scratch.exclude()).unwrap(), b"# local ignore\n");
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(GIT_PRE_COMMIT_PATH).exists());
    assert!(
        !repository
            .common_dir()
            .join(GIT_COMMIT_MESSAGE_PATH)
            .exists()
    );
    assert!(
        !repository
            .common_dir()
            .join(GIT_REFERENCE_TRANSACTION_PATH)
            .exists()
    );
    assert!(!repository.common_dir().join(ENTRY_PATH).exists());
    assert!(!repository.common_dir().join(SKILL_PATH).exists());
}

#[test]
fn apply_is_idempotent_and_unapply_is_exact() {
    let scratch = Scratch::new("round-trip");
    let repository = scratch.repository();
    let binary = scratch.binary(b"#!/bin/sh\nprintf 'spice-appliance\\n'\n");
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
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_PRE_COMMIT_PATH)).unwrap(),
        GIT_PRE_COMMIT
    );
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_COMMIT_MESSAGE_PATH)).unwrap(),
        GIT_COMMIT_MESSAGE
    );
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_REFERENCE_TRANSACTION_PATH)).unwrap(),
        GIT_REFERENCE_TRANSACTION
    );
    assert_eq!(
        mode(&repository.common_dir().join(GIT_PRE_COMMIT_PATH)),
        EXECUTABLE_MODE
    );
    assert_eq!(
        mode(&repository.common_dir().join(GIT_COMMIT_MESSAGE_PATH)),
        EXECUTABLE_MODE
    );
    assert_eq!(
        mode(&repository.common_dir().join(GIT_REFERENCE_TRANSACTION_PATH)),
        EXECUTABLE_MODE
    );
    assert_eq!(
        run(&repository.common_dir().join(BINARY_PATH)),
        b"spice-appliance\n"
    );
    assert!(
        fs::read_to_string(scratch.exclude())
            .unwrap()
            .contains(EXCLUDE_BLOCK)
    );
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(SKILL_PATH)).unwrap(),
        SKILL
    );
    assert_eq!(
        record_path(&repository).unwrap(),
        repository
            .admit_state()
            .unwrap()
            .runtime()
            .join(RECORD_NAME)
    );
    assert_eq!(mode(&record_path(&repository).unwrap()), RECORD_MODE);
    assert!(
        !repository
            .common_dir()
            .join(".spice/rust/install.json")
            .exists()
    );

    let removed = unapply(&repository).expect("reverse exact install");
    let repeated_remove = unapply(&repository).expect("repeat exact reversal");
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert_eq!(removed, repeated_remove);
    assert_eq!(fs::read(scratch.exclude()).unwrap(), before_exclude);
    for operation in planned.operations() {
        let observed = observe(&operation.destination(&repository))
            .unwrap()
            .map(|observed| observed.image);
        assert_eq!(observed, operation.previous().cloned());
    }
    assert!(record_path(&repository).unwrap().is_file());
}

/// A terminal receipt may describe fixed bytes from the build that removed them.
///
/// The recorded skill identity is changed to stand for that earlier build and
/// the optional binary is changed on disk before this build previews. Neither
/// historical identity may authorize the new application: the terminal record
/// first has to prove exact restoration, then preview and apply derive a fresh
/// plan whose digest names this build's skill and binary. Reversing that new
/// install proves the supersession retained the old receipt's only remaining
/// authority instead of simply ignoring it.
#[test]
fn an_uninstalled_prior_build_receipt_is_superseded_by_a_current_digest_bound_install() {
    let scratch = Scratch::new("build-supersession");
    let repository = scratch.repository();
    let binary = scratch.binary(b"prior build binary\n");
    let prior = plan(&repository, Some(&binary)).expect("prior build plans");
    apply(&repository, Some(&binary), &prior.digest().unwrap()).expect("prior build applies");
    unapply(&repository).expect("prior build restores every target");

    rewrite_recorded_asset(
        &repository,
        Asset::Skill,
        DATA_MODE,
        b"prior build embedded skill\n",
    );
    fs::write(&binary, b"current build binary\n").expect("current binary replaces prior input");

    let historical = read_record(&repository)
        .expect("terminal historical receipt is readable")
        .expect("terminal receipt remains")
        .record;
    assert_eq!(historical.status, InstallStatus::Uninstalled);
    let historical_skill = historical
        .plan
        .operations
        .iter()
        .find(|operation| operation.asset == Asset::Skill)
        .expect("historical plan carries its skill")
        .generated
        .clone();

    let current = plan(&repository, Some(&binary)).expect("current build previews over terminal");
    let current_skill = current
        .operations()
        .iter()
        .find(|operation| operation.asset() == Asset::Skill)
        .expect("current plan carries its skill")
        .generated();
    assert_ne!(current_skill, &historical_skill);
    assert_eq!(
        current_skill,
        &GeneratedImage::new(DATA_MODE, SKILL.as_bytes())
    );
    let current_digest = current.digest().expect("current plan has an identity");
    assert_ne!(current_digest, historical.plan_digest);

    let installed = apply(&repository, Some(&binary), &current_digest)
        .expect("current digest supersedes the terminal receipt");
    assert_eq!(installed.status(), InstallStatus::Complete);
    assert_eq!(installed.digest(), &current_digest);
    assert_eq!(
        fs::read(repository.common_dir().join(SKILL_PATH)).unwrap(),
        SKILL.as_bytes()
    );
    assert_eq!(
        fs::read(repository.common_dir().join(BINARY_PATH)).unwrap(),
        b"current build binary\n"
    );

    let removed = unapply(&repository).expect("current install remains exactly reversible");
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert!(current.operations().iter().all(|operation| {
        observe(&operation.destination(&repository))
            .unwrap()
            .map(|observed| observed.image)
            == operation.previous().cloned()
    }));
}

/// Only a terminal uninstalled receipt may carry another build's fixed bytes.
///
/// Each live status is produced through its real progress route before its
/// recorded skill identity is changed. Planning must refuse at record admission
/// and leave the partially applied, complete, or partially restored targets
/// exactly as they were.
#[test]
fn every_nonterminal_receipt_remains_bound_to_current_build_assets() {
    for status in [
        InstallStatus::Applying,
        InstallStatus::Complete,
        InstallStatus::Unapplying,
    ] {
        let scratch = Scratch::new(&format!("current-build-{status:?}"));
        let repository = scratch.repository();
        let planned = plan(&repository, None).expect("current build plans");
        let digest = planned.digest().unwrap();
        match status {
            InstallStatus::Applying => {
                assert!(matches!(
                    apply_with_interruption(&repository, None, &digest, Some(0)),
                    Err(InstallError::Interrupted { operation: 0 })
                ));
            }
            InstallStatus::Complete => {
                apply(&repository, None, &digest).expect("current build applies");
            }
            InstallStatus::Unapplying => {
                apply(&repository, None, &digest).expect("current build applies");
                let last = planned.operations().len() - 1;
                assert!(matches!(
                    unapply_with_interruption(&repository, Some(last)),
                    Err(InstallError::Interrupted { operation }) if operation == last
                ));
            }
            InstallStatus::Uninstalled => unreachable!("the terminal exception is tested above"),
        }
        assert_eq!(
            read_record(&repository)
                .unwrap()
                .expect("nonterminal record remains")
                .record
                .status,
            status
        );
        let before = images(&repository, &planned);

        rewrite_recorded_asset(
            &repository,
            Asset::Skill,
            DATA_MODE,
            b"foreign build embedded skill\n",
        );

        let error = plan(&repository, None).expect_err("foreign live receipt must refuse");
        assert!(
            matches!(error, InstallError::InvalidRecord(ref reason)
                if reason == "generated skill differs from this build"),
            "{status:?} refused for the wrong reason: {error}"
        );
        assert_eq!(images(&repository, &planned), before);
    }
}

/// The build digest still refuses a foreign reference-transaction hook now that
/// the hook reaches a published executable rather than a build system.
///
/// Keeping the hook as text is what keeps this arm reachable. Only the
/// executable it execs is compiled, and that artifact is published by explicit
/// activation instead of being embedded here, so `include_str!` still has bytes
/// to compare against and a receipt carrying anything else is still refused. An
/// arm that had quietly become unreachable would look exactly like this one
/// without a test to tell them apart.
///
/// The bytes written in are the wrapper this hook replaced, which is the
/// tampering with a motive: a receipt claiming the build still shells out to a
/// build system under the ref lock is precisely what must not be honoured.
#[test]
fn a_foreign_reference_transaction_hook_is_refused() {
    let scratch = Scratch::new("reference-transaction-build");
    let repository = scratch.repository();
    let planned = plan(&repository, None).expect("current build plans");
    apply(&repository, None, &planned.digest().unwrap()).expect("current build applies");
    let before = images(&repository, &planned);

    rewrite_recorded_asset(
        &repository,
        Asset::GitReferenceTransaction,
        EXECUTABLE_MODE,
        b"#!/usr/bin/env sh\nexec cargo xtask reference-transaction \"$1\"\n",
    );

    let error = plan(&repository, None).expect_err("a foreign hook receipt must refuse");
    assert!(
        matches!(error, InstallError::InvalidRecord(ref reason)
            if reason == "Git reference-transaction hook differs from this build"),
        "refused for the wrong reason: {error}"
    );
    assert_eq!(images(&repository, &planned), before);
}

#[test]
fn preserved_exclusions_keep_their_prior_mode() {
    let scratch = Scratch::new("preserve");
    let repository = scratch.repository();
    set_test_mode(&scratch.exclude(), 0o600);

    let planned = plan(&repository, None).unwrap();
    let receipt = apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert_eq!(mode(&scratch.exclude()), 0o600);

    unapply(&repository).unwrap();
    assert_eq!(
        receipt.plan().operations()[0].previous().unwrap().mode(),
        0o600
    );
    assert_eq!(mode(&scratch.exclude()), 0o600);
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
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(ENTRY_PATH).exists());
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
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(BINARY_PATH).exists());
}

#[test]
fn refresh_publishes_the_rebuilt_executable_without_withdrawing_anything() {
    let scratch = Scratch::new("refresh-window");
    let repository = scratch.repository();
    let first = vec![0x5a; LARGE_BINARY_BYTES];
    let second = vec![0xa5; LARGE_BINARY_BYTES];
    let binary = scratch.binary(&first);
    let installed = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    apply(&repository, Some(&binary), &installed).unwrap();
    fs::write(&binary, &second).unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);

    let observer = watch_installed_tree(&repository);
    let refreshed = plan(&repository, Some(&binary)).expect("a rebuilt executable plans a refresh");
    let receipt =
        apply(&repository, Some(&binary), &refreshed.digest().unwrap()).expect("apply the refresh");
    let observed = observer.stop();

    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(
        fs::read(repository.common_dir().join(BINARY_PATH)).unwrap(),
        second
    );
    assert!(
        observed.withdrawn.is_empty(),
        "the refresh withdrew {:?}",
        observed.withdrawn
    );
    // Without this the emptiness above is vacuous: an observer that ran
    // entirely before or after the swap would report the same nothing.
    assert!(
        observed.binary_images.contains(&first) && observed.binary_images.contains(&second),
        "the observer never spanned the swap: {} samples over {} distinct images",
        observed.samples,
        observed.binary_images.len()
    );
}

#[test]
fn reversing_before_reinstalling_withdraws_what_a_refresh_keeps() {
    let scratch = Scratch::new("refresh-control");
    let repository = scratch.repository();
    let first = vec![0x5a; LARGE_BINARY_BYTES];
    let second = vec![0xa5; LARGE_BINARY_BYTES];
    let binary = scratch.binary(&first);
    let installed = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    apply(&repository, Some(&binary), &installed).unwrap();
    fs::write(&binary, &second).unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);

    // The upgrade the refresh replaces, watched by the same observer: it has to
    // report the window, or its silence next door proves nothing.
    let observer = watch_installed_tree(&repository);
    unapply(&repository).expect("reverse the install");
    let rebuilt = plan(&repository, Some(&binary)).expect("plan a fresh install");
    apply(&repository, Some(&binary), &rebuilt.digest().unwrap()).expect("reinstall");
    let observed = observer.stop();

    assert!(
        observed.withdrawn.contains(BINARY_PATH),
        "the observer cannot see a withdrawn executable at all: {} samples",
        observed.samples
    );
    assert!(observed.withdrawn.contains(GIT_PRE_COMMIT_PATH));
}

#[test]
fn reversing_a_refresh_restores_the_tree_the_first_install_found() {
    let scratch = Scratch::new("refresh-reversal");
    let repository = scratch.repository();
    let binary = scratch.binary(b"old binary bytes");
    let before_exclude = fs::read(scratch.exclude()).unwrap();
    let installed = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    apply(&repository, Some(&binary), &installed).unwrap();
    fs::write(&binary, b"new binary bytes").unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);

    let refreshed = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &refreshed.digest().unwrap()).unwrap();
    let receipt = unapply(&repository).expect("reverse the refreshed install");

    // A refresh inherits its reversal images rather than rebasing them onto its
    // own output, so reversing one uninstalls instead of downgrading.
    assert_eq!(receipt.status(), InstallStatus::Uninstalled);
    assert_eq!(fs::read(scratch.exclude()).unwrap(), before_exclude);
    for path in PUBLISHED_PATHS {
        assert!(
            !repository.common_dir().join(path).exists(),
            "{path} survived the reversal"
        );
    }
}

#[test]
fn generated_guidance_and_install_errors_name_spice() {
    assert_eq!(
        SKILL
            .matches("--git-common-dir)/.spice-install/bin/spice up")
            .count(),
        1
    );
    assert!(!SKILL.contains("spice-rust"));
    assert!(!SKILL.contains("Rust"));

    for error in [
        InstallError::BinaryEmpty(PathBuf::from("fixture")),
        InstallError::BinaryNotRegular(PathBuf::from("fixture")),
        InstallError::BinaryContextChanged,
        InstallError::DivergentExclusions,
        InstallError::RecordMissing,
        InstallError::ApplyInProgress,
        InstallError::UnapplyInProgress,
    ] {
        let message = error.to_string();
        assert!(message.contains("Spice"), "{message}");
        assert!(!message.contains("Rust"), "{message}");
        assert!(!message.contains("spice-rust"), "{message}");
    }
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
    const INTERRUPTION: usize = 2;

    let scratch = Scratch::new("unapply-resume");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();

    let error = unapply_with_interruption(&repository, Some(INTERRUPTION)).unwrap_err();
    assert!(matches!(
        error,
        InstallError::Interrupted {
            operation: INTERRUPTION
        }
    ));
    let interrupted = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(interrupted.status, InstallStatus::Unapplying);
    assert_eq!(
        interrupted.completed,
        planned.operations().len() - 1 - INTERRUPTION
    );
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
    fs::write(
        repository.common_dir().join(ENTRY_PATH),
        b"operator-owned replacement\n",
    )
    .unwrap();

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
    fs::create_dir_all(record_path(&repository).unwrap().parent().unwrap()).unwrap();
    fs::write(
        record_path(&repository).unwrap(),
        b"{\"unexpected\":true}\n",
    )
    .unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::Decode(_)));
    assert!(!repository.common_dir().join(ENTRY_PATH).exists());

    let divergent = Scratch::new("divergent");
    let repository = divergent.repository();
    fs::write(divergent.exclude(), format!("{EXCLUDE_BEGIN}\nwrong\n")).unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::DivergentExclusions));
    assert!(!record_path(&repository).unwrap().exists());

    let duplicate = Scratch::new("duplicate-block");
    let repository = duplicate.repository();
    fs::write(
        duplicate.exclude(),
        format!("{EXCLUDE_BLOCK}{EXCLUDE_BLOCK}"),
    )
    .unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(error, InstallError::DivergentExclusions));
    assert!(!record_path(&repository).unwrap().exists());
}

#[test]
fn legacy_record_names_its_owning_release_and_refuses_without_mutation() {
    for (relative, owner) in LEGACY_RECORDS {
        let scratch = Scratch::new(owner);
        let repository = scratch.repository();
        let legacy = repository.common_dir().join(relative);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, b"legacy record bytes\n").unwrap();
        let before_exclude = fs::read(scratch.exclude()).unwrap();

        let error = plan(&repository, None).unwrap_err();
        assert!(matches!(
            &error,
            InstallError::LegacyRecord { path, owner: observed }
                if path == &legacy && observed == &owner
        ));
        assert!(error.to_string().contains(owner));
        assert_eq!(fs::read(scratch.exclude()).unwrap(), before_exclude);
        assert!(!record_path(&repository).unwrap().exists());
        assert!(!repository.common_dir().join(ENTRY_PATH).exists());
    }
}

#[test]
fn prior_current_path_record_names_its_owning_release_before_mutation() {
    for (version, owner) in LEGACY_RECORD_VERSIONS {
        let scratch = Scratch::new(owner);
        let repository = scratch.repository();
        let planned = plan(&repository, None).unwrap();
        apply(&repository, None, &planned.digest().unwrap()).unwrap();
        let installed = images(&repository, &planned);
        let mut record = read_record(&repository).unwrap().unwrap().record;
        record.version = version;
        let mut bytes = wire::encode(&record).unwrap();
        if version == 4 {
            let mut document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let worktree = serde_json::to_value(repository.worktree_root()).unwrap();
            document["repository"]["worktree"] = worktree.clone();
            document["plan"]["repository"]["worktree"] = worktree;
            bytes = serde_json::to_vec(&document).unwrap();
        }
        fs::write(record_path(&repository).unwrap(), bytes).unwrap();

        let error = plan(&repository, None).unwrap_err();
        assert!(matches!(
            &error,
            InstallError::LegacyRecord { path, owner: observed }
                if path == &record_path(&repository).unwrap() && observed == &owner
        ));
        assert!(error.to_string().contains(owner));
        assert_eq!(images(&repository, &planned), installed);
    }
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
    fs::write(
        record_path(&repository).unwrap(),
        wire::encode(&record).unwrap(),
    )
    .unwrap();

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
    let outside = repository.common_dir().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, repository.common_dir().join(".spice-install")).unwrap();

    let planned = plan(&repository, None).unwrap();
    let skill = planned
        .operations()
        .iter()
        .find(|operation| operation.asset() == Asset::Skill)
        .unwrap();
    assert_eq!(skill.destination, "outside/agents/skills/spice/SKILL.md");
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(outside.join("agents/skills/spice/SKILL.md").is_file());
    unapply(&repository).unwrap();

    let escaped = Scratch::new("symlink-escape");
    let repository = escaped.repository();
    let external = escaped.0.with_extension("external");
    fs::create_dir(&external).unwrap();
    let canonical_external = fs::canonicalize(&external).unwrap();
    symlink(&external, repository.common_dir().join(".spice-install")).unwrap();
    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        error,
        InstallError::PathOutsideRoot(path) if path == canonical_external
    ));
    assert!(
        plan(&repository, None)
            .unwrap_err()
            .to_string()
            .contains("canonical permitted root")
    );
    assert!(fs::read_dir(&external).unwrap().next().is_none());
    fs::remove_dir(external).unwrap();

    let state = Scratch::new("state-symlink");
    let repository = state.repository();
    let outside = state.join("outside-state");
    fs::create_dir(&outside).unwrap();
    let state_root = repository
        .common_dir()
        .join(crate::repository::STATE_DIRECTORY);
    symlink(&outside, &state_root).unwrap();

    let error = plan(&repository, None).unwrap_err();
    assert!(matches!(
        error,
        InstallError::State(crate::repository::StateError::Linked { path })
            if path == state_root
    ));
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}

/// Every path an install publishes and a refresh must never withdraw.
const PUBLISHED_PATHS: [&str; 7] = [
    GIT_PRE_COMMIT_PATH,
    GIT_COMMIT_MESSAGE_PATH,
    GIT_REFERENCE_TRANSACTION_PATH,
    ENTRY_PATH,
    STATIC_PATH,
    SKILL_PATH,
    BINARY_PATH,
];

/// What a concurrent reader saw of the installed tree while it was changing.
#[derive(Default)]
struct ObservedTree {
    samples: usize,
    withdrawn: BTreeSet<String>,
    binary_images: BTreeSet<Vec<u8>>,
}

struct TreeObserver {
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<ObservedTree>,
}

impl TreeObserver {
    fn stop(self) -> ObservedTree {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.join().expect("the tree observer finishes")
    }
}

/// Read every published path in a tight loop until stopped.
///
/// The loop never sleeps, so a withdrawal that spans more than a few syscalls
/// is seen rather than stepped over; `reversing_before_reinstalling_withdraws_
/// what_a_refresh_keeps` is the control that this reader can report one.
fn watch_installed_tree(repository: &Repository) -> TreeObserver {
    let common = repository.common_dir().to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        let mut observed = ObservedTree::default();
        while !signal.load(Ordering::Relaxed) {
            observed.samples += 1;
            for path in PUBLISHED_PATHS {
                match fs::read(common.join(path)) {
                    Ok(bytes) => {
                        if path == BINARY_PATH {
                            observed.binary_images.insert(bytes);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        observed.withdrawn.insert(path.to_owned());
                    }
                    Err(error) => panic!("observe {path}: {error}"),
                }
            }
        }
        observed
    });
    TreeObserver { stop, worker }
}

fn images(repository: &Repository, plan: &InstallPlan) -> Vec<Option<ObservedFile>> {
    plan.operations()
        .iter()
        .map(|operation| observe(&operation.destination(repository)).unwrap())
        .collect()
}

/// Rewrite only the fixed skill identity and rebind the record's own digest.
///
/// The record remains canonical and digest-consistent, exactly as another build
/// would encode it; no malformed-record refusal can satisfy the tests that call
/// this helper.
fn rewrite_recorded_asset(repository: &Repository, asset: Asset, mode: u32, bytes: &[u8]) {
    let mut record = read_record(repository)
        .expect("current record reads before the simulated build boundary")
        .expect("an install record exists")
        .record;
    let operation = record
        .plan
        .operations
        .iter_mut()
        .find(|operation| operation.asset == asset)
        .expect("the canonical plan carries the rewritten asset");
    let historical = GeneratedImage::new(mode, bytes);
    assert_ne!(operation.generated, historical);
    operation.generated = historical;
    record.plan_digest = record.plan.digest().expect("changed plan has a digest");
    fs::write(
        record_path(repository).expect("record path resolves"),
        wire::encode(&record).expect("changed record encodes canonically"),
    )
    .expect("changed build receipt installs");
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

    fn binary(&self, bytes: &[u8]) -> PathBuf {
        self.executable("build/spice", bytes)
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
