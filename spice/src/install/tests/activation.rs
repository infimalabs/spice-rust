use super::*;

#[test]
fn hook_activation_previews_every_worktree_switches_last_and_reverses_exactly() {
    let (scratch, linked, repository, binary) = linked_hook_fixture();
    let planned = plan(&repository, Some(&binary)).expect("preview linked hook activation");
    let activation = planned
        .hook_activation()
        .expect("a product install carries hook activation");
    assert_eq!(activation.observations().len(), 2);
    assert_eq!(activation.mutations().len(), 2);
    assert_eq!(
        activation.mutations().last().unwrap().scope(),
        HookConfigurationScope::Local,
    );
    assert_eq!(
        git_values(
            scratch.path(),
            &["config", "--local", "--get-all", "core.hooksPath"]
        ),
        [".githooks"]
    );
    assert_eq!(
        git_values(
            &linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        ),
        [".githooks"]
    );
    assert!(!record_path(&repository).unwrap().exists());

    let digest = planned.digest().unwrap();
    let installed = apply(&repository, Some(&binary), &digest).expect("apply hook activation");
    let hooks = installed_git_hooks(&repository)
        .to_str()
        .expect("fixture hooks path is UTF-8")
        .to_owned();
    assert_installed_hook_selection(&scratch, &linked, &hooks);
    assert_eq!(
        apply(&repository, Some(&binary), &digest).expect("repeat activation"),
        installed,
    );
    refresh_tolerated_hook_selection(&scratch, &linked, &repository, &binary, &digest, &hooks);
    fs::write(&binary, b"#!/bin/sh\nexit 1\n").unwrap();
    set_test_mode(&binary, EXECUTABLE_MODE);
    let refresh = plan(&repository, Some(&binary)).expect("refresh linked activation");
    assert_eq!(refresh.hook_activation().unwrap().mutations().len(), 2);
    apply(&repository, Some(&binary), &refresh.digest().unwrap())
        .expect("apply linked activation refresh");

    unapply(&repository).expect("reverse hook activation");
    assert_prior_hook_selection(&scratch, &linked);
}

fn linked_hook_fixture() -> (Scratch, PathBuf, Repository, PathBuf) {
    let scratch = Scratch::new("hook-activation");
    let linked = scratch.join("linked");
    git_ok(
        scratch.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    git_ok(
        scratch.path(),
        &["config", "extensions.worktreeConfig", "true"],
    );
    git_ok(
        scratch.path(),
        &["config", "--local", "--add", "core.hooksPath", ".githooks"],
    );
    git_ok(
        &linked,
        &[
            "config",
            "--worktree",
            "--add",
            "core.hooksPath",
            ".githooks",
        ],
    );
    let repository = scratch.repository();
    let binary = scratch.binary(b"#!/bin/sh\nexit 0\n");
    (scratch, linked, repository, binary)
}

fn assert_installed_hook_selection(scratch: &Scratch, linked: &Path, hooks: &str) {
    for root in [scratch.path(), linked] {
        assert_eq!(
            git_values(root, &["config", "--get", "core.hooksPath"]),
            [hooks],
        );
    }
    assert!(
        git_values(
            linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        )
        .is_empty()
    );
}

fn refresh_tolerated_hook_selection(
    scratch: &Scratch,
    linked: &Path,
    repository: &Repository,
    binary: &Path,
    digest: &Digest,
    hooks: &str,
) {
    git_ok(
        scratch.path(),
        &["config", "--local", "core.hooksPath", ".githooks"],
    );
    git_ok(
        linked,
        &["config", "--worktree", "core.hooksPath", ".githooks"],
    );
    let selection_refresh =
        plan(repository, Some(binary)).expect("refresh tolerated hook selections");
    assert_ne!(
        selection_refresh.digest().unwrap().as_str(),
        digest.as_str()
    );
    assert!(
        selection_refresh
            .hook_activation()
            .unwrap()
            .mutations()
            .iter()
            .all(|mutation| mutation
                .superseded()
                .is_some_and(|values| { values.len() == 1 && values[0] == ".githooks" }))
    );
    apply(
        repository,
        Some(binary),
        &selection_refresh.digest().unwrap(),
    )
    .expect("converge tolerated hook selections");
    assert_installed_hook_selection(scratch, linked, hooks);
}

fn assert_prior_hook_selection(scratch: &Scratch, linked: &Path) {
    assert_eq!(
        git_values(
            scratch.path(),
            &["config", "--local", "--get-all", "core.hooksPath"]
        ),
        [".githooks"]
    );
    assert_eq!(
        git_values(
            linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        ),
        [".githooks"]
    );
}

#[test]
fn foreign_linked_hooks_refuse_preview_without_install_mutation() {
    let scratch = Scratch::new("foreign-hooks");
    let linked = scratch.join("linked");
    git_ok(
        scratch.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    git_ok(
        scratch.path(),
        &["config", "extensions.worktreeConfig", "true"],
    );
    git_ok(
        &linked,
        &[
            "config",
            "--worktree",
            "core.hooksPath",
            ".spice/python-hooks",
        ],
    );
    let repository = scratch.repository();
    let binary = scratch.binary(b"fixture binary\n");
    let canonical_linked = fs::canonicalize(&linked).unwrap();

    let error = plan(&repository, Some(&binary)).expect_err("foreign hooks must refuse");

    assert!(matches!(
        error,
        InstallError::ForeignHookPath { worktree, .. } if worktree == canonical_linked
    ));
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(".spice-install").exists());
    assert_eq!(
        git_values(
            &linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        ),
        [".spice/python-hooks"],
    );
}

#[test]
fn mixed_tolerated_hook_values_refuse_preview_without_install_mutation() {
    let scratch = Scratch::new("mixed-hooks");
    let linked = scratch.join("linked");
    git_ok(
        scratch.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    git_ok(
        scratch.path(),
        &["config", "extensions.worktreeConfig", "true"],
    );
    git_ok(
        scratch.path(),
        &["config", "--local", "core.hooksPath", ".githooks"],
    );
    let repository = scratch.repository();
    let target = installed_git_hooks(&repository)
        .to_str()
        .expect("fixture hook path is UTF-8")
        .to_owned();
    git_ok(
        &linked,
        &["config", "--worktree", "core.hooksPath", &target],
    );
    let binary = scratch.binary(b"fixture binary\n");

    let error = plan(&repository, Some(&binary)).expect_err("mixed hook values must refuse");

    assert!(matches!(
        error,
        InstallError::ForeignHookPath { observed, .. }
            if observed == [".githooks", target.as_str()]
    ));
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(".spice-install").exists());
}

#[test]
fn a_linked_worktree_added_after_install_enters_the_next_activation_plan() {
    let scratch = Scratch::new("late-linked-hooks");
    git_ok(
        scratch.path(),
        &["config", "extensions.worktreeConfig", "true"],
    );
    let repository = scratch.repository();
    let binary = scratch.binary(b"#!/bin/sh\nexit 0\n");
    let first = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &first.digest().unwrap()).unwrap();
    let linked = scratch.join("linked");
    git_ok(
        scratch.path(),
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let target = installed_git_hooks(&repository)
        .to_str()
        .expect("fixture hook path is UTF-8")
        .to_owned();
    git_ok(
        &linked,
        &["config", "--worktree", "core.hooksPath", &target],
    );

    let expanded = plan(&repository, Some(&binary)).expect("re-preflight late linked worktree");
    assert_ne!(expanded.digest().unwrap(), first.digest().unwrap());
    assert_eq!(expanded.hook_activation().unwrap().observations().len(), 2);
    apply(&repository, Some(&binary), &expanded.digest().unwrap())
        .expect("activate hooks in late linked worktree");
    assert!(
        git_values(
            &linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        )
        .is_empty()
    );
    unapply(&repository).unwrap();
    assert_eq!(
        git_values(
            &linked,
            &["config", "--worktree", "--get-all", "core.hooksPath"]
        ),
        [target],
    );
}

#[test]
fn hook_configuration_movement_refuses_a_stale_digest_before_file_publication() {
    let scratch = Scratch::new("hook-stale");
    let repository = scratch.repository();
    let binary = scratch.binary(b"fixture binary\n");
    let stale = plan(&repository, Some(&binary)).unwrap().digest().unwrap();
    git_ok(
        scratch.path(),
        &["config", "--local", "core.hooksPath", ".githooks"],
    );

    let error = apply(&repository, Some(&binary), &stale).unwrap_err();

    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert!(!record_path(&repository).unwrap().exists());
    assert!(!repository.common_dir().join(".spice-install").exists());
    assert_eq!(
        git_values(
            scratch.path(),
            &["config", "--local", "--get", "core.hooksPath"]
        ),
        [".githooks"],
    );
}

#[test]
fn hook_configuration_stage_resumes_both_directions_after_publication() {
    let scratch = Scratch::new("hook-resume");
    let repository = scratch.repository();
    let binary = scratch.binary(b"fixture binary\n");
    let planned = plan(&repository, Some(&binary)).unwrap();
    let digest = planned.digest().unwrap();
    let hook_step = planned.operations().len() + planned.retirements().len();

    let error = apply_with_interruption(&repository, Some(&binary), &digest, Some(hook_step))
        .expect_err("interrupt after hook configuration publication");
    assert!(matches!(error, InstallError::Interrupted { operation } if operation == hook_step));
    let applying = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(applying.status, InstallStatus::Applying);
    assert_eq!(applying.completed, hook_step);
    hook_activation::verify(planned.hook_activation().unwrap(), true).unwrap();
    apply(&repository, Some(&binary), &digest).expect("resume activation");

    let error = unapply_with_interruption(&repository, Some(planned.operation_count() - 1))
        .expect_err("interrupt after hook configuration reversal");
    assert!(
        matches!(error, InstallError::Interrupted { operation } if operation == planned.operation_count() - 1)
    );
    let unapplying = read_record(&repository).unwrap().unwrap().record;
    assert_eq!(unapplying.status, InstallStatus::Unapplying);
    assert_eq!(unapplying.completed, 0);
    hook_activation::verify(planned.hook_activation().unwrap(), false).unwrap();
    assert_eq!(
        unapply(&repository).expect("resume reversal").status(),
        InstallStatus::Uninstalled,
    );
}

#[test]
fn relocation_is_repaired_only_by_reinstalling_the_absolute_hook_plan() {
    let mut scratch = Scratch::new("relocation");
    let repository = scratch.repository();
    let binary = scratch.binary(b"fixture binary\n");
    let original = plan(&repository, Some(&binary)).unwrap();
    apply(&repository, Some(&binary), &original.digest().unwrap()).unwrap();
    let old_hooks = installed_git_hooks(&repository)
        .to_string_lossy()
        .into_owned();
    let old_root = scratch.0.clone();
    let moved_root = old_root.with_extension("moved");
    assert!(!moved_root.exists());
    fs::rename(&old_root, &moved_root).expect("relocate installed repository");
    scratch.0.clone_from(&moved_root);
    let moved = scratch.repository();
    let moved_binary = scratch.join("build/spice");

    assert_eq!(
        git_values(scratch.path(), &["config", "--get", "core.hooksPath"]),
        [old_hooks],
        "relocation itself does not hide or rewrite the absolute selection",
    );
    let refreshed =
        plan(&moved, Some(&moved_binary)).expect("reinstall previews relocation repair");
    let new_hooks = installed_git_hooks(&moved).to_string_lossy().into_owned();
    assert_ne!(refreshed.digest().unwrap(), original.digest().unwrap());
    assert_eq!(refreshed.hook_activation().unwrap().hooks_path(), new_hooks);
    apply(&moved, Some(&moved_binary), &refreshed.digest().unwrap())
        .expect("reinstall repairs relocation");
    assert_eq!(
        git_values(scratch.path(), &["config", "--get", "core.hooksPath"]),
        [new_hooks],
    );
    unapply(&moved).expect("relocated receipt remains reversible");
    assert!(git_values(scratch.path(), &["config", "--get-all", "core.hooksPath"]).is_empty());
}
