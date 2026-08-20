use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use super::*;

mod activation;
mod audit;
mod migration;
mod refresh;
mod safety;

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
            Asset::GitPreCommit,
            Asset::GitCommitMessage,
            Asset::GitReferenceTransaction,
            Asset::HookEntry,
            Asset::HookStatic,
            Asset::HookWrapperSpice,
            Asset::Skill,
            Asset::Binary,
        ]
    );
    assert!(first.operations().iter().all(|operation| {
        !matches!(
            operation.path(),
            RETIRED_XTASK_PATH | RETIRED_XTASK_GUARD_PATH
        )
    }));
    assert_eq!(
        first
            .retirements()
            .iter()
            .map(Retirement::path)
            .collect::<Vec<_>>(),
        [RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH],
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
    assert!(!repository.common_dir().join(WRAPPER_SPICE_PATH).exists());
    assert!(!repository.common_dir().join(SKILL_PATH).exists());
}

#[test]
fn no_planned_operation_escapes_the_common_directory() {
    let scratch = Scratch::new("scope");
    let repository = scratch.repository();
    let binary = scratch.binary(&[0x5a; 64]);

    // Read the contract directly, so the assertion below is reachable even
    // for an asset whose worktree-scoped destination cannot be planned at all.
    for asset in [
        Asset::GitPreCommit,
        Asset::GitCommitMessage,
        Asset::GitReferenceTransaction,
        Asset::HookEntry,
        Asset::HookStatic,
        Asset::HookWrapperSpice,
        Asset::Skill,
        Asset::Binary,
    ] {
        assert_eq!(
            asset_contract(asset).0,
            Location::CommonGit,
            "{asset:?} is contracted against a worktree, which a \
             common-directory-scoped record cannot rediscover"
        );
    }

    let derived = plan(&repository, Some(&binary)).expect("derive install plan");

    // A plan holding no operations would satisfy the sweep below vacuously.
    assert!(
        derived.operations().len() >= 8,
        "the scope sweep needs the whole asset inventory; found {}",
        derived.operations().len()
    );
    for operation in derived.operations() {
        assert_eq!(
            operation.location(),
            Location::CommonGit,
            "{:?} resolves against a worktree, which a common-directory-scoped \
             record cannot rediscover",
            operation.asset()
        );
    }
}

#[test]
fn a_worktree_scoped_identity_is_refused() {
    let scratch = Scratch::new("identity");
    let repository = scratch.repository();
    let binary = scratch.binary(&[0x5a; 64]);
    let mut derived = plan(&repository, Some(&binary)).expect("derive install plan");

    // Without this the refusal below could come from an already-invalid plan.
    derived
        .validate()
        .expect("the derived plan validates as built");

    derived.repository.worktree = Some(repository.worktree_root().to_path_buf());
    let error = derived
        .validate()
        .expect_err("a worktree-scoped identity is refused");

    assert!(
        matches!(
            &error,
            InstallError::InvalidRecord(detail)
                if detail.contains("not common-directory scoped")
        ),
        "{error}"
    );
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
    let installed_images = images(&repository, &planned);
    let repeated = apply(&repository, Some(&binary), &digest).expect("repeat exact apply");

    assert_eq!(first.status(), InstallStatus::Complete);
    assert_eq!(first.completed(), planned.operation_count());
    assert_eq!(first, repeated);
    assert_eq!(installed_images, images(&repository, &planned));
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_PRE_COMMIT_PATH)).unwrap(),
        String::from_utf8(
            git_hooks::launcher(repository.common_dir(), git_hooks::GitHook::PreCommit).unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_COMMIT_MESSAGE_PATH)).unwrap(),
        String::from_utf8(
            git_hooks::launcher(repository.common_dir(), git_hooks::GitHook::CommitMessage)
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        fs::read_to_string(repository.common_dir().join(GIT_REFERENCE_TRANSACTION_PATH)).unwrap(),
        String::from_utf8(
            git_hooks::launcher(
                repository.common_dir(),
                git_hooks::GitHook::ReferenceTransaction,
            )
            .unwrap()
        )
        .unwrap()
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
    assert_eq!(fs::read(scratch.exclude()).unwrap(), b"# local ignore\n");
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

#[test]
fn inspection_admits_only_a_current_receipt_and_its_exact_destinations() {
    let scratch = Scratch::new("inspect");
    let repository = scratch.repository();
    assert_eq!(inspect(&repository).expect("inspect absent receipt"), None);

    let planned = plan(&repository, None).expect("plan install");
    let receipt = apply(&repository, None, &planned.digest().unwrap()).expect("apply install");
    assert_eq!(
        inspect(&repository).expect("inspect current install"),
        Some(receipt)
    );

    fs::write(
        repository.common_dir().join(ENTRY_PATH),
        b"operator hook replacement\n",
    )
    .expect("drift installed hook");
    assert!(matches!(
        inspect(&repository).expect_err("inspection refuses drift"),
        InstallError::Drift { path, .. } if path == ENTRY_PATH
    ));
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

/// A live receipt without an installed executable remains current-build bound.
///
/// Each live status is produced through its real progress route before its
/// recorded skill identity is changed. Planning must refuse at record admission
/// and leave the partially applied, binary-less complete, or partially restored
/// targets exactly as they were. The completed appliance exception is tested
/// separately with the installed binary that anchors its prior build.
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
/// the hook reaches the installed product rather than a build system.
///
/// Keeping the hook as text is what keeps this arm reachable. Only the
/// executable it execs is compiled, and that artifact is published by the same
/// digest-bound installation, so `include_str!` still has bytes
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
fn stale_plan_refuses_before_record_or_target_mutation() {
    let scratch = Scratch::new("stale");
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    let stale = planned.digest().unwrap();
    let target = repository.common_dir().join(GIT_PRE_COMMIT_PATH);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, b"operator changed this\n").unwrap();

    let error = apply(&repository, None, &stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(fs::read(target).unwrap(), b"operator changed this\n");
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

/// Historical admission does not turn a rewritten receipt into authority.
/// The on-disk skill still belongs to the original receipt, so changing only
/// its recorded digest is detected as drift before a refresh plan can exist.
#[test]
fn a_rewritten_complete_receipt_cannot_invent_a_prior_build_asset() {
    let scratch = Scratch::new("refresh-prior-tamper");
    let repository = scratch.repository();
    let binary = scratch.binary(b"installed build\n");
    let installed = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &installed.digest().unwrap()).unwrap();
    let before = images(&repository, &installed);

    rewrite_recorded_asset(
        &repository,
        Asset::Skill,
        DATA_MODE,
        b"receipt-only invented skill\n",
    );
    let rewritten_record = fs::read(record_path(&repository).unwrap()).unwrap();
    let error = plan(&repository, Some(&binary))
        .expect_err("a receipt-only build rewrite must not authorize refresh");

    assert!(matches!(error, InstallError::Drift { path, .. } if path == SKILL_PATH));
    assert_eq!(
        fs::read(record_path(&repository).unwrap()).unwrap(),
        rewritten_record
    );
    assert_eq!(images(&repository, &installed), before);
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
    // Twice, because a second refresh is where an inherited reversal image
    // could quietly rebase onto the first refresh's own output.
    fs::write(&binary, b"newer binary bytes").unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);
    let again = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &again.digest().unwrap()).unwrap();
    assert_eq!(
        fs::read(repository.common_dir().join(BINARY_PATH)).unwrap(),
        b"newer binary bytes"
    );
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
fn checkout_skill_source_matches_the_embedded_install_asset() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("agent/SKILL.md");

    assert_eq!(fs::read_to_string(source).unwrap(), SKILL);
}

#[test]
fn generated_guidance_and_install_errors_name_spice() {
    for command in ["`spice session briefing`", "`spice task status`"] {
        assert!(
            SKILL.contains(command),
            "missing bootstrap command {command}"
        );
    }
    assert!(SKILL.contains("## Operator Decision Boundary"));
    assert!(SKILL.contains("`spice config trust grant` / `revoke` plan"));
    assert!(!SKILL.contains("spice-rust"));
    assert!(!SKILL.contains("Rust"));
    assert!(!SKILL.contains("`spice agent"));

    for error in [
        InstallError::BinaryEmpty(PathBuf::from("fixture")),
        InstallError::BinaryNotRegular(PathBuf::from("fixture")),
        InstallError::BinaryContextChanged,
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
    assert_eq!(completed.completed(), planned.operation_count());
    assert!(planned.operations().iter().all(|operation| {
        let observed = observe(&operation.destination(&repository)).unwrap();
        matches_generated(observed.as_ref(), operation)
    }));
}

/// Every path an install publishes and a refresh must never withdraw.
const PUBLISHED_PATHS: [&str; 8] = [
    GIT_PRE_COMMIT_PATH,
    GIT_COMMIT_MESSAGE_PATH,
    GIT_REFERENCE_TRANSACTION_PATH,
    ENTRY_PATH,
    STATIC_PATH,
    WRAPPER_SPICE_PATH,
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

/// Replace one completed receipt asset and its installed image coherently.
///
/// This is the state a previous build really leaves, unlike
/// [`rewrite_recorded_asset`], which deliberately changes only the claim.
fn install_completed_historical_asset(
    repository: &Repository,
    asset: Asset,
    mode: u32,
    bytes: &[u8],
) -> GeneratedImage {
    let mut record = read_record(repository)
        .expect("current record reads before the simulated build boundary")
        .expect("an install record exists")
        .record;
    assert_eq!(record.status, InstallStatus::Complete);
    assert!(
        binary_operation(&record.plan).is_some(),
        "only an installed executable anchors prior-build fixed assets"
    );
    let index = record
        .plan
        .operations
        .iter()
        .position(|operation| operation.asset == asset)
        .expect("the canonical plan carries the historical asset");
    let destination = record.plan.operations[index].destination(repository);
    fs::write(&destination, bytes).expect("historical installed asset writes");
    set_test_mode(&destination, mode);
    let observed = observe(&destination)
        .expect("historical installed asset reads")
        .expect("historical installed asset exists");
    let historical = observed.generated();
    record.plan.operations[index].generated = historical.clone();
    record.installed[index] = Some(observed.generation);
    record.plan_digest = record.plan.digest().expect("historical plan has a digest");
    fs::write(
        record_path(repository).expect("record path resolves"),
        wire::encode(&record).expect("historical record encodes canonically"),
    )
    .expect("historical build receipt installs");
    historical
}

fn run(path: &Path) -> Vec<u8> {
    let output = Command::new(path)
        .output()
        .expect("invoke fixture executable");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    output.stdout
}

fn git_ok(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("launch fixture Git command");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn git_values(root: &Path, arguments: &[&str]) -> Vec<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("launch fixture Git query");
    if output.status.code() == Some(1) {
        return Vec::new();
    }
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("fixture Git query is UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
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
                    let initialized = Command::new("git")
                        .args(["init", "-q", "-b", "main"])
                        .current_dir(&root)
                        .status()
                        .expect("launch fixture Git initialization");
                    assert!(initialized.success());
                    let configured = Command::new("git")
                        .args(["config", "user.email", "install@example.invalid"])
                        .current_dir(&root)
                        .status()
                        .expect("configure fixture Git email");
                    assert!(configured.success());
                    let configured = Command::new("git")
                        .args(["config", "user.name", "Install Fixture"])
                        .current_dir(&root)
                        .status()
                        .expect("configure fixture Git name");
                    assert!(configured.success());
                    let committed = Command::new("git")
                        .args(["commit", "--allow-empty", "-qm", "fixture"])
                        .current_dir(&root)
                        .status()
                        .expect("commit fixture Git root");
                    assert!(committed.success());
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

    fn path(&self) -> &Path {
        &self.0
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
